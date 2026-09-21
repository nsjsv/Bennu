use std::ffi::OsString;
use std::fmt;
use std::num::NonZeroU32;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use file_search::SearchRuntimeIdentity;
use tokio::process::Command;

use super::bounded_child_output::read_bounded_child_output;

const SEARCH_CONTROL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(1);
const MAXIMUM_SYSTEMCTL_STDOUT_BYTES: usize = 64 * 1024;
const MAXIMUM_SYSTEMCTL_STDERR_BYTES: usize = 16 * 1024;
const MAXIMUM_SYSTEMD_SOURCE_FIELD_CHARS: usize = 4096;
const MAXIMUM_SYSTEMD_EXEC_START_FIELD_CHARS: usize = 16 * 1024;
const REQUIRED_MEMORY_HIGH: u64 = 512_000_000;
const REQUIRED_MEMORY_MAX: u64 = 640_000_000;
const REQUIRED_MEMORY_SWAP_MAX: u64 = 0;
const REQUIRED_SERVICE_SLICE: &str = "background.slice";
const MAXIMUM_SUPPORTED_BASE_PAGE_BYTES: u64 = 65_536;
const PACKAGED_RELEASE_FRAGMENT_PATH: &str = "/usr/lib/systemd/user/bennu-search.service";
const PACKAGED_RELEASE_EXEC_START_PATH: &str = "/usr/lib/bennu/bennu-searchd";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SearchUnitAction {
    Show,
    DaemonReload,
    Start,
    Restart,
    KillControlGroup,
    ResetFailed,
}

impl SearchUnitAction {
    pub(super) fn arguments(self, runtime_identity: SearchRuntimeIdentity) -> Vec<OsString> {
        let arguments = match self {
            Self::Show => vec![
                "--user",
                "--no-pager",
                "--property=ActiveState",
                "--property=SubState",
                "--property=MainPID",
                "--property=ControlGroup",
                "--property=MemoryHigh",
                "--property=MemoryMax",
                "--property=MemorySwapMax",
                "--property=Slice",
                "--property=CPUQuotaPerSecUSec",
                "--property=Result",
                "--property=ExecMainStatus",
                "--property=NRestarts",
                "--property=FragmentPath",
                "--property=DropInPaths",
                "--property=ExecStart",
                "show",
            ],
            Self::DaemonReload => vec!["--user", "--no-pager", "daemon-reload"],
            Self::Start => vec!["--user", "--no-pager", "--no-block", "start"],
            Self::Restart => vec!["--user", "--no-pager", "--no-block", "restart"],
            Self::KillControlGroup => vec![
                "--user",
                "--no-pager",
                "--signal=SIGKILL",
                "--kill-whom=all",
                "kill",
            ],
            Self::ResetFailed => vec!["--user", "--no-pager", "reset-failed"],
        };
        let mut arguments = arguments
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        if self != Self::DaemonReload {
            arguments.push(OsString::from(runtime_identity.systemd_unit()));
        }
        arguments
    }

    fn name(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::DaemonReload => "daemon-reload",
            Self::Start => "start",
            Self::Restart => "restart",
            Self::KillControlGroup => "kill",
            Self::ResetFailed => "reset-failed",
        }
    }
}

pub(super) struct SearchUnitController {
    pub(super) runtime_identity: SearchRuntimeIdentity,
    pub(super) systemctl_executable: PathBuf,
    pub(super) cgroup_root: PathBuf,
}

impl SearchUnitController {
    pub(super) fn system(runtime_identity: SearchRuntimeIdentity) -> Self {
        Self {
            runtime_identity,
            systemctl_executable: PathBuf::from("systemctl"),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
        }
    }

    pub(super) async fn execute(&self, action: SearchUnitAction) -> Result<String, String> {
        let systemd_unit = self.runtime_identity.systemd_unit();
        let mut command = Command::new(&self.systemctl_executable);
        command
            .args(action.arguments(self.runtime_identity))
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            format!(
                "could not execute systemctl --user {} {}: {error}",
                action.name(),
                systemd_unit
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "systemctl stdout pipe was unavailable".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "systemctl stderr pipe was unavailable".to_owned())?;
        let command_result = tokio::time::timeout(SEARCH_CONTROL_ATTEMPT_TIMEOUT, async move {
            let (status_result, stdout_result, stderr_result) = tokio::join!(
                child.wait(),
                read_bounded_child_output(stdout, MAXIMUM_SYSTEMCTL_STDOUT_BYTES),
                read_bounded_child_output(stderr, MAXIMUM_SYSTEMCTL_STDERR_BYTES),
            );
            Ok::<_, std::io::Error>((status_result?, stdout_result?, stderr_result?))
        })
        .await
        .map_err(|_| {
            format!(
                "systemctl --user {} {} timed out",
                action.name(),
                systemd_unit
            )
        })?
        .map_err(|error| {
            format!(
                "could not collect systemctl --user {} {} output: {error}",
                action.name(),
                systemd_unit
            )
        })?;
        let (command_status, stdout_output, stderr_output) = command_result;

        if stdout_output.exceeded_limit {
            return Err(format!(
                "systemctl --user {} {} stdout exceeded {} bytes",
                action.name(),
                systemd_unit,
                MAXIMUM_SYSTEMCTL_STDOUT_BYTES
            ));
        }
        if stderr_output.exceeded_limit {
            return Err(format!(
                "systemctl --user {} {} stderr exceeded {} bytes",
                action.name(),
                systemd_unit,
                MAXIMUM_SYSTEMCTL_STDERR_BYTES
            ));
        }

        if !command_status.success() {
            let stderr = String::from_utf8_lossy(&stderr_output.bytes)
                .trim()
                .to_owned();
            let detail = if stderr.is_empty() {
                command_status.to_string()
            } else {
                stderr
            };
            return Err(format!(
                "systemctl --user {} {} failed: {detail}",
                action.name(),
                systemd_unit
            ));
        }

        Ok(String::from_utf8_lossy(&stdout_output.bytes).into_owned())
    }

    pub(super) async fn show(&self) -> Result<SearchUnitSnapshot, String> {
        SearchUnitSnapshot::parse(
            &self.execute(SearchUnitAction::Show).await?,
            self.runtime_identity,
        )
    }

    pub(super) async fn reload_then_execute(&self, action: SearchUnitAction) -> Result<(), String> {
        self.execute(SearchUnitAction::DaemonReload).await?;
        self.execute(action).await?;
        Ok(())
    }

    async fn validate_effective_cgroup(&self, snapshot: &SearchUnitSnapshot) -> Result<(), String> {
        let relative_control_group = snapshot
            .control_group
            .as_ref()
            .expect("ready service snapshot has a ControlGroup")
            .strip_prefix("/")
            .expect("validated ControlGroup is absolute");
        let cgroup_directory = self.cgroup_root.join(relative_control_group);

        for (file_name, required_value) in [
            ("memory.high", REQUIRED_MEMORY_HIGH),
            ("memory.max", REQUIRED_MEMORY_MAX),
        ] {
            let limit_path = cgroup_directory.join(file_name);
            let limit_text = read_effective_cgroup_setting(&limit_path, snapshot).await?;
            let effective_value = limit_text.trim().parse::<u64>().map_err(|error| {
                format!(
                    "effective cgroup limit {} is invalid: {error}; {}",
                    limit_path.display(),
                    snapshot.description()
                )
            })?;
            // Linux memcg 会按基础页向下取整；这里只接受不会放宽配置上限的小幅取整。
            let rounding_difference = required_value.saturating_sub(effective_value);
            let is_safe_kernel_rounding = effective_value <= required_value
                && rounding_difference < MAXIMUM_SUPPORTED_BASE_PAGE_BYTES;
            if !is_safe_kernel_rounding {
                return Err(format!(
                    "effective cgroup limit {}={} is not a safe page-rounded value for {}; {}",
                    limit_path.display(),
                    effective_value,
                    required_value,
                    snapshot.description()
                ));
            }
        }

        let swap_limit_path = cgroup_directory.join("memory.swap.max");
        let swap_limit_text = read_effective_cgroup_setting(&swap_limit_path, snapshot).await?;
        let effective_swap_limit = swap_limit_text.trim().parse::<u64>().map_err(|error| {
            format!(
                "effective cgroup limit {} is invalid: {error}; {}",
                swap_limit_path.display(),
                snapshot.description()
            )
        })?;
        if effective_swap_limit != REQUIRED_MEMORY_SWAP_MAX {
            return Err(format!(
                "effective cgroup limit {}={} does not equal required value {}; {}",
                swap_limit_path.display(),
                effective_swap_limit,
                REQUIRED_MEMORY_SWAP_MAX,
                snapshot.description()
            ));
        }

        validate_optional_unbounded_cpu_max(&cgroup_directory.join("cpu.max"), snapshot).await?;
        Ok(())
    }

    pub(super) async fn validated_main_pid(
        &self,
        snapshot: &SearchUnitSnapshot,
    ) -> Result<NonZeroU32, String> {
        let main_pid = snapshot.ready_main_pid()?;
        self.validate_effective_cgroup(snapshot).await?;
        Ok(main_pid)
    }
}

async fn validate_optional_unbounded_cpu_max(
    cpu_max_path: &Path,
    snapshot: &SearchUnitSnapshot,
) -> Result<(), String> {
    let cpu_max_text = match tokio::fs::read_to_string(cpu_max_path).await {
        Ok(cpu_max_text) => cpu_max_text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "could not read effective cgroup limit {}: {error}; {}",
                cpu_max_path.display(),
                snapshot.description()
            ));
        }
    };
    let cpu_max_fields = cpu_max_text.split_whitespace().collect::<Vec<_>>();
    let [quota_text, period_text] = cpu_max_fields.as_slice() else {
        return Err(format!(
            "effective cgroup limit {} must contain quota and period; {}",
            cpu_max_path.display(),
            snapshot.description()
        ));
    };
    let period = period_text.parse::<u64>().map_err(|error| {
        format!(
            "effective cgroup period {} is invalid: {error}; {}",
            cpu_max_path.display(),
            snapshot.description()
        )
    })?;
    if *quota_text != "max" || period == 0 {
        return Err(format!(
            "effective cgroup limit {}={} {} is not unbounded; {}",
            cpu_max_path.display(),
            quota_text,
            period_text,
            snapshot.description()
        ));
    }
    Ok(())
}

async fn read_effective_cgroup_setting(
    setting_path: &Path,
    snapshot: &SearchUnitSnapshot,
) -> Result<String, String> {
    tokio::fs::read_to_string(setting_path)
        .await
        .map_err(|error| {
            format!(
                "could not read effective cgroup limit {}: {error}; {}",
                setting_path.display(),
                snapshot.description()
            )
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UnitActiveState {
    Active,
    Activating,
    Inactive,
    Failed,
    Deactivating,
    Other(String),
}

impl UnitActiveState {
    fn parse(value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err("systemd reported empty ActiveState".to_owned());
        }
        Ok(match value {
            "active" => Self::Active,
            "activating" => Self::Activating,
            "inactive" => Self::Inactive,
            "failed" => Self::Failed,
            "deactivating" => Self::Deactivating,
            value => Self::Other(value.to_owned()),
        })
    }
}

impl fmt::Display for UnitActiveState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Active => formatter.write_str("active"),
            Self::Activating => formatter.write_str("activating"),
            Self::Inactive => formatter.write_str("inactive"),
            Self::Failed => formatter.write_str("failed"),
            Self::Deactivating => formatter.write_str("deactivating"),
            Self::Other(value) => formatter.write_str(value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchUnitDefinitionSource {
    Expected,
    UserOverride,
    Unexpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SearchUnitSnapshot {
    pub(super) runtime_identity: SearchRuntimeIdentity,
    pub(super) active_state: UnitActiveState,
    pub(super) sub_state: String,
    pub(super) main_pid: Option<NonZeroU32>,
    pub(super) control_group: Option<PathBuf>,
    pub(super) memory_high: u64,
    pub(super) memory_max: u64,
    pub(super) memory_swap_max: u64,
    pub(super) service_slice: String,
    pub(super) cpu_quota_per_sec_usec: String,
    pub(super) service_result: String,
    pub(super) exec_main_status: u32,
    pub(super) restart_count: u64,
    pub(super) fragment_path: String,
    pub(super) drop_in_paths: String,
    pub(super) exec_start_path: String,
}

impl SearchUnitSnapshot {
    pub(super) fn parse(
        command_stdout: &str,
        runtime_identity: SearchRuntimeIdentity,
    ) -> Result<Self, String> {
        let mut active_state = None;
        let mut sub_state = None;
        let mut main_pid = None;
        let mut main_pid_observed = false;
        let mut control_group = None;
        let mut memory_high = None;
        let mut memory_max = None;
        let mut memory_swap_max = None;
        let mut service_slice = None;
        let mut cpu_quota_per_sec_usec = None;
        let mut service_result = None;
        let mut exec_main_status = None;
        let mut restart_count = None;
        let mut fragment_path = None;
        let mut drop_in_paths = None;
        let mut exec_start_path = None;

        for property_line in command_stdout.lines() {
            let Some((property_name, property_value)) = property_line.split_once('=') else {
                return Err("systemctl show returned a malformed property line".to_owned());
            };
            let property_name = property_name.trim();
            let property_value = property_value.trim();
            let maximum_property_chars = if property_name == "ExecStart" {
                MAXIMUM_SYSTEMD_EXEC_START_FIELD_CHARS
            } else {
                MAXIMUM_SYSTEMD_SOURCE_FIELD_CHARS
            };
            validate_systemd_property_value(property_name, property_value, maximum_property_chars)?;
            match property_name {
                "ActiveState" => replace_once(
                    &mut active_state,
                    UnitActiveState::parse(property_value)?,
                    property_name,
                )?,
                "SubState" => {
                    if property_value.is_empty() {
                        return Err("systemd reported empty SubState".to_owned());
                    }
                    replace_once(&mut sub_state, property_value.to_owned(), property_name)?;
                }
                "MainPID" => {
                    let parsed_pid = property_value
                        .parse::<u32>()
                        .map_err(|error| format!("systemd reported invalid MainPID: {error}"))?;
                    if main_pid_observed {
                        return Err("systemctl show repeated MainPID".to_owned());
                    }
                    main_pid = NonZeroU32::new(parsed_pid);
                    main_pid_observed = true;
                }
                "ControlGroup" => replace_once(
                    &mut control_group,
                    Self::parse_control_group(property_value)?,
                    property_name,
                )?,
                "MemoryHigh" => replace_once(
                    &mut memory_high,
                    parse_u64_property(property_name, property_value)?,
                    property_name,
                )?,
                "MemoryMax" => replace_once(
                    &mut memory_max,
                    parse_u64_property(property_name, property_value)?,
                    property_name,
                )?,
                "MemorySwapMax" => replace_once(
                    &mut memory_swap_max,
                    parse_u64_property(property_name, property_value)?,
                    property_name,
                )?,
                "Slice" => {
                    if property_value.is_empty() {
                        return Err("systemd reported empty Slice".to_owned());
                    }
                    replace_once(&mut service_slice, property_value.to_owned(), property_name)?;
                }
                "CPUQuotaPerSecUSec" => {
                    if property_value.is_empty() {
                        return Err("systemd reported empty CPUQuotaPerSecUSec".to_owned());
                    }
                    replace_once(
                        &mut cpu_quota_per_sec_usec,
                        property_value.to_owned(),
                        property_name,
                    )?;
                }
                "Result" => {
                    if property_value.is_empty() {
                        return Err("systemd reported empty Result".to_owned());
                    }
                    replace_once(
                        &mut service_result,
                        property_value.to_owned(),
                        property_name,
                    )?;
                }
                "ExecMainStatus" => replace_once(
                    &mut exec_main_status,
                    parse_u32_property(property_name, property_value)?,
                    property_name,
                )?,
                "NRestarts" => replace_once(
                    &mut restart_count,
                    parse_u64_property(property_name, property_value)?,
                    property_name,
                )?,
                "FragmentPath" => {
                    replace_once(&mut fragment_path, property_value.to_owned(), property_name)?
                }
                "DropInPaths" => {
                    replace_once(&mut drop_in_paths, property_value.to_owned(), property_name)?
                }
                "ExecStart" => replace_once(
                    &mut exec_start_path,
                    parse_exec_start_path(property_value)?,
                    property_name,
                )?,
                _ => {}
            }
        }

        if !main_pid_observed {
            return Err("systemctl show omitted MainPID".to_owned());
        }
        Ok(Self {
            runtime_identity,
            active_state: required_property(active_state, "ActiveState")?,
            sub_state: required_property(sub_state, "SubState")?,
            main_pid,
            control_group: required_property(control_group, "ControlGroup")?,
            memory_high: required_property(memory_high, "MemoryHigh")?,
            memory_max: required_property(memory_max, "MemoryMax")?,
            memory_swap_max: required_property(memory_swap_max, "MemorySwapMax")?,
            service_slice: required_property(service_slice, "Slice")?,
            cpu_quota_per_sec_usec: required_property(
                cpu_quota_per_sec_usec,
                "CPUQuotaPerSecUSec",
            )?,
            service_result: required_property(service_result, "Result")?,
            exec_main_status: required_property(exec_main_status, "ExecMainStatus")?,
            restart_count: required_property(restart_count, "NRestarts")?,
            fragment_path: required_property(fragment_path, "FragmentPath")?,
            drop_in_paths: required_property(drop_in_paths, "DropInPaths")?,
            exec_start_path: required_property(exec_start_path, "ExecStart")?,
        })
    }

    fn parse_control_group(property_value: &str) -> Result<Option<PathBuf>, String> {
        if property_value.is_empty() {
            return Ok(None);
        }
        let control_group = PathBuf::from(property_value);
        let mut components = control_group.components();
        let has_absolute_root = components.next() == Some(Component::RootDir);
        let has_safe_first_segment = matches!(components.next(), Some(Component::Normal(_)));
        let remaining_segments_are_safe =
            components.all(|component| matches!(component, Component::Normal(_)));
        if !has_absolute_root || !has_safe_first_segment || !remaining_segments_are_safe {
            return Err(format!(
                "systemd reported unsafe ControlGroup={property_value}"
            ));
        }
        Ok(Some(control_group))
    }

    pub(super) fn ready_main_pid(&self) -> Result<NonZeroU32, String> {
        if self.active_state != UnitActiveState::Active || self.sub_state != "running" {
            return Err(format!(
                "search service unit is not active/running: {}",
                self.description()
            ));
        }
        if self.control_group.is_none() {
            return Err(format!(
                "active/running search service unit reported an empty ControlGroup: {}",
                self.description()
            ));
        }
        if self.memory_high != REQUIRED_MEMORY_HIGH
            || self.memory_max != REQUIRED_MEMORY_MAX
            || self.memory_swap_max != REQUIRED_MEMORY_SWAP_MAX
        {
            return Err(format!(
                "search service unit effective memory properties do not match the required envelope: {}",
                self.description()
            ));
        }
        if self.service_slice != REQUIRED_SERVICE_SLICE || self.cpu_quota_per_sec_usec != "infinity"
        {
            return Err(format!(
                "search service unit does not use the required unbounded background CPU policy: {}",
                self.description()
            ));
        }
        if self.service_result != "success" || self.exec_main_status != 0 || self.restart_count != 0
        {
            return Err(format!(
                "search service unit has failed or restarted: {}",
                self.description()
            ));
        }
        self.main_pid.ok_or_else(|| {
            format!(
                "active/running search service unit reported MainPID=0: {}",
                self.description()
            )
        })
    }

    pub(super) fn main_pid(&self) -> Option<NonZeroU32> {
        self.main_pid
    }

    pub(super) fn may_have_processes(&self) -> bool {
        self.main_pid.is_some()
            || !matches!(
                &self.active_state,
                UnitActiveState::Inactive | UnitActiveState::Failed
            )
    }

    pub(super) fn description(&self) -> String {
        let control_group = display_optional_path(self.control_group.as_deref());
        let fragment_path = display_optional_text(&self.fragment_path);
        let drop_in_paths = display_optional_text(&self.drop_in_paths);
        let exec_start_path = display_optional_text(&self.exec_start_path);
        let facts = format!(
            "Unit={}, ActiveState={}, SubState={}, MainPID={}, ControlGroup={}, MemoryHigh={}, MemoryMax={}, MemorySwapMax={}, Slice={}, CPUQuotaPerSecUSec={}, Result={}, ExecMainStatus={}, NRestarts={}, FragmentPath={}, DropInPaths={}, ExecStartPath={}",
            self.runtime_identity.systemd_unit(),
            self.active_state,
            self.sub_state,
            self.main_pid.map_or(0, NonZeroU32::get),
            control_group,
            self.memory_high,
            self.memory_max,
            self.memory_swap_max,
            self.service_slice,
            self.cpu_quota_per_sec_usec,
            self.service_result,
            self.exec_main_status,
            self.restart_count,
            fragment_path,
            drop_in_paths,
            exec_start_path,
        );
        match self.definition_source() {
            SearchUnitDefinitionSource::Expected => facts,
            SearchUnitDefinitionSource::UserOverride => format!(
                "{facts}; unit source guidance: a user unit definition or drop-in overrides the expected service; remove or migrate the FragmentPath/DropInPaths shown above, run systemctl --user daemon-reload, then retry"
            ),
            SearchUnitDefinitionSource::Unexpected => format!(
                "{facts}; unit source guidance: FragmentPath/ExecStartPath do not match the current bundle; {}",
                unexpected_definition_action(self.runtime_identity)
            ),
        }
    }

    fn definition_source(&self) -> SearchUnitDefinitionSource {
        if !self.drop_in_paths.is_empty() {
            return SearchUnitDefinitionSource::UserOverride;
        }
        if self.runtime_identity == SearchRuntimeIdentity::Release
            && expected_user_fragment(SearchRuntimeIdentity::Release)
                .is_some_and(|path| path == Path::new(&self.fragment_path))
        {
            return SearchUnitDefinitionSource::UserOverride;
        }
        if self.matches_expected_definition() {
            SearchUnitDefinitionSource::Expected
        } else {
            SearchUnitDefinitionSource::Unexpected
        }
    }

    fn matches_expected_definition(&self) -> bool {
        match self.runtime_identity {
            SearchRuntimeIdentity::Release => {
                self.fragment_path == PACKAGED_RELEASE_FRAGMENT_PATH
                    && self.exec_start_path == PACKAGED_RELEASE_EXEC_START_PATH
            }
            SearchRuntimeIdentity::Development => {
                expected_user_fragment(SearchRuntimeIdentity::Development)
                    .is_some_and(|path| path == Path::new(&self.fragment_path))
                    && expected_development_exec_start()
                        .is_some_and(|path| path == Path::new(&self.exec_start_path))
            }
        }
    }
}

fn validate_systemd_property_value(
    property_name: &str,
    property_value: &str,
    maximum_chars: usize,
) -> Result<(), String> {
    if property_value.chars().count() > maximum_chars {
        return Err(format!(
            "systemd reported {property_name} longer than {maximum_chars} characters"
        ));
    }
    if property_value.chars().any(char::is_control) {
        return Err(format!(
            "systemd reported {property_name} containing control characters"
        ));
    }
    Ok(())
}

fn parse_exec_start_path(property_value: &str) -> Result<String, String> {
    if property_value.is_empty() {
        return Ok(String::new());
    }
    let Some(path_start) = property_value.find("path=") else {
        return Ok("<unrecognized>".to_owned());
    };
    let path_value = &property_value[path_start + "path=".len()..];
    let path_end = path_value
        .find(';')
        .or_else(|| path_value.find('}'))
        .unwrap_or(path_value.len());
    let executable_path = path_value[..path_end].trim();
    if executable_path.is_empty() {
        return Err("systemd reported ExecStart with an empty executable path".to_owned());
    }
    validate_systemd_property_value(
        "ExecStart executable path",
        executable_path,
        MAXIMUM_SYSTEMD_SOURCE_FIELD_CHARS,
    )?;
    Ok(executable_path.to_owned())
}

fn replace_once<T>(target: &mut Option<T>, value: T, property_name: &str) -> Result<(), String> {
    if target.replace(value).is_some() {
        return Err(format!("systemctl show repeated {property_name}"));
    }
    Ok(())
}

fn required_property<T>(property: Option<T>, property_name: &str) -> Result<T, String> {
    property.ok_or_else(|| format!("systemctl show omitted {property_name}"))
}

fn parse_u64_property(property_name: &str, property_value: &str) -> Result<u64, String> {
    property_value
        .parse::<u64>()
        .map_err(|error| format!("systemd reported invalid {property_name}: {error}"))
}

fn parse_u32_property(property_name: &str, property_value: &str) -> Result<u32, String> {
    property_value
        .parse::<u32>()
        .map_err(|error| format!("systemd reported invalid {property_name}: {error}"))
}

fn display_optional_path(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "<none>".to_owned())
}

fn display_optional_text(value: &str) -> &str {
    if value.is_empty() {
        "<none>"
    } else {
        value
    }
}

fn expected_user_fragment(runtime_identity: SearchRuntimeIdentity) -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("systemd/user")
            .join(runtime_identity.systemd_unit()),
    )
}

fn expected_development_exec_start() -> Option<PathBuf> {
    let application_path = std::env::current_exe().ok()?;
    let install_prefix = application_path.parent()?.parent()?;
    Some(install_prefix.join("lib/bennu/bennu-searchd"))
}

fn unexpected_definition_action(runtime_identity: SearchRuntimeIdentity) -> &'static str {
    match runtime_identity {
        SearchRuntimeIdentity::Release => {
            "reinstall the current Bennu package, run systemctl --user daemon-reload, then retry"
        }
        SearchRuntimeIdentity::Development => {
            "run scripts/install-bennu-dev.sh install --yes to restore the managed development unit, then retry"
        }
    }
}

#[cfg(test)]
#[path = "search_service_systemd_tests.rs"]
mod tests;
