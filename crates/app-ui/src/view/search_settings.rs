use file_search::{
    IndexHealth, IndexPhase, IndexedQueryAvailability, SearchPathConfigurationPhase,
    SearchRootAvailability, SearchServicePhase, SearchServiceStatus,
};
use iced::widget::{button, column, container, row, text_input, tooltip, Column};
use iced::{Alignment, Element, Length};

use crate::app::FileBrowser;
use crate::appearance::{
    context_menu_button_style, navigation_icon_button_style, navigation_text_input_style,
};
use crate::formatting::{format_file_size, format_middle_ellipsized_text, format_system_time};
use crate::icons::IconSymbol;
use crate::model::{
    Message, ScrollbarRegion, ScrollbarViewport, ScrollbarVisibility, SearchEndpointState,
    SearchPathEntryKind, SearchServiceDiagnosticKind, SearchServiceIncident,
    SearchServiceIncidentState, SearchServiceRecoveryAction, SearchServiceRecoveryState,
};
use crate::typography::{localized_text, readable_text};

use super::auxiliary_window_layout::auxiliary_detail_scroller;
use super::option_controls::destructive_confirmation_button_style;
use super::settings_group::{
    action_setting_row, info_setting_row, labeled_setting_row, settings_group, toggle_setting_row,
    SETTINGS_GROUP_SPACING,
};
use super::settings_window::chrome_top_spacer;
use super::{themed_icon, IconTone};

pub(super) fn search_settings_detail(
    browser: &FileBrowser,
    scrollbar_visibility: ScrollbarVisibility,
    scrollbar_viewport: Option<ScrollbarViewport>,
) -> Element<'_, Message> {
    let content = column![
        chrome_top_spacer(browser),
        settings_group("Indexed locations", indexed_location_rows(browser)),
        settings_group("Excluded locations", excluded_location_rows(browser)),
        settings_group("Content indexing", content_indexing_rows(browser)),
        settings_group("Status overview", status_overview_rows(browser)),
        settings_group("Index progress", index_progress_rows(browser)),
        settings_group("Recent issues", recent_issue_rows(browser)),
        settings_group("Service recovery", service_recovery_rows(browser)),
    ]
    .spacing(SETTINGS_GROUP_SPACING)
    .width(Length::Fill);

    auxiliary_detail_scroller(
        content,
        ScrollbarRegion::Settings,
        scrollbar_visibility,
        scrollbar_viewport,
        Message::SettingsScrolled,
    )
}

fn indexed_location_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let settings = &browser.search_service.path_settings;
    if settings.confirmed.is_none() {
        return vec![status_notice("Loading indexed locations...")];
    }

    let mut rows = path_configuration_state_rows(browser);
    if settings.draft.custom_roots.is_empty() {
        rows.push(status_notice(
            "Home is indexed by default. Add another location here.",
        ));
    } else {
        rows.extend(settings.draft.custom_roots.iter().map(|path| {
            search_path_row(
                path,
                SearchPathEntryKind::CustomRoot,
                configured_root_status(browser, path),
            )
        }));
    }
    rows.push(search_path_input_row(
        SearchPathEntryKind::CustomRoot,
        &settings.custom_root_input,
        settings.picker_in_flight.is_some(),
    ));
    if let Some(notice) = path_picker_failure_notice(browser, SearchPathEntryKind::CustomRoot) {
        rows.push(notice);
    }
    rows
}

fn excluded_location_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let settings = &browser.search_service.path_settings;
    if settings.confirmed.is_none() {
        return vec![status_notice("Loading excluded locations...")];
    }

    let mut rows = Vec::new();
    if settings.draft.exclusions.is_empty() {
        rows.push(status_notice("No additional locations are excluded."));
    } else {
        rows.extend(settings.draft.exclusions.iter().map(|path| {
            search_path_row(path, SearchPathEntryKind::Exclusion, "Excluded".to_owned())
        }));
    }
    rows.push(search_path_input_row(
        SearchPathEntryKind::Exclusion,
        &settings.exclusion_input,
        settings.picker_in_flight.is_some(),
    ));
    if let Some(notice) = path_picker_failure_notice(browser, SearchPathEntryKind::Exclusion) {
        rows.push(notice);
    }
    rows
}

fn path_picker_failure_notice(
    browser: &FileBrowser,
    kind: SearchPathEntryKind,
) -> Option<Element<'static, Message>> {
    browser
        .search_service
        .path_settings
        .picker_failure
        .as_ref()
        .filter(|(failed_kind, _)| *failed_kind == kind)
        .map(|(_, message)| status_notice(message.clone()))
}

fn path_configuration_state_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let settings = &browser.search_service.path_settings;
    if settings.apply_in_flight {
        return vec![status_notice("Applying search location changes...")];
    }
    if let Some(message) = settings.failure.as_ref() {
        return vec![path_configuration_failure_row(message)];
    }
    let Some(status) = browser
        .search_service
        .confirmed_status
        .as_ref()
        .and_then(|service| service.index_status.as_ref())
        .map(|index| &index.path_configuration)
    else {
        return Vec::new();
    };
    match &status.phase {
        SearchPathConfigurationPhase::Ready => Vec::new(),
        SearchPathConfigurationPhase::Applying => {
            vec![status_notice("Applying search location changes...")]
        }
        SearchPathConfigurationPhase::Failed { message } => {
            vec![path_configuration_failure_row(message)]
        }
    }
}

fn path_configuration_failure_row(message: &str) -> Element<'_, Message> {
    info_setting_row(
        row![
            localized_text(message.to_owned())
                .size(11)
                .width(Length::Fill),
            button(readable_text("Retry").size(11))
                .on_press(Message::SearchPathConfigurationRetryPressed)
                .padding([5, 9])
                .style(context_menu_button_style()),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into(),
    )
}

fn configured_root_status(browser: &FileBrowser, path: &std::path::Path) -> String {
    let Some(root) = browser
        .search_service
        .confirmed_status
        .as_ref()
        .and_then(|service| service.index_status.as_ref())
        .and_then(|index| {
            index
                .path_configuration
                .roots
                .iter()
                .find(|root| root.path == path)
        })
    else {
        return "Waiting for status".to_owned();
    };
    match &root.availability {
        SearchRootAvailability::Available => "Available".to_owned(),
        SearchRootAvailability::Unavailable { message } => {
            format!("Unavailable: {message}")
        }
        SearchRootAvailability::MountChanged { message } => {
            format!("Storage changed: {message}")
        }
    }
}

fn search_path_row<'a>(
    path: &'a std::path::Path,
    kind: SearchPathEntryKind,
    status: String,
) -> Element<'a, Message> {
    let remove_path = path.to_path_buf();
    info_setting_row(
        row![
            column![
                localized_text(format_middle_ellipsized_text(&path.to_string_lossy(), 72,))
                    .size(12)
                    .width(Length::Fill),
                localized_text(status).size(10).width(Length::Fill),
            ]
            .spacing(2)
            .width(Length::Fill),
            tooltip(
                button(themed_icon(IconSymbol::Trash, IconTone::Normal, 12.0))
                    .on_press(Message::SearchPathEntryRemoved(kind, remove_path))
                    .padding([7, 8])
                    .style(navigation_icon_button_style()),
                path_tooltip("Remove location"),
                tooltip::Position::Left,
            ),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into(),
    )
}

fn search_path_input_row(
    kind: SearchPathEntryKind,
    value: &str,
    picker_in_flight: bool,
) -> Element<'_, Message> {
    let input = text_input(
        &crate::localization::translate_current("Absolute path"),
        value,
    )
    .on_input(move |value| Message::SearchPathInputChanged(kind, value))
    .on_submit(Message::SearchPathInputCommitted(kind))
    .padding([7, 9])
    .size(12)
    .style(navigation_text_input_style)
    .width(Length::Fill);
    let chooser = button(themed_icon(IconSymbol::FolderOpen, IconTone::Normal, 13.0))
        .padding([8, 9])
        .style(navigation_icon_button_style());
    let chooser = if picker_in_flight {
        chooser
    } else {
        chooser.on_press(Message::SearchPathDirectoryChooserPressed(kind))
    };
    info_setting_row(
        row![
            input,
            tooltip(
                chooser,
                path_tooltip("Choose directory"),
                tooltip::Position::Left,
            ),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into(),
    )
}

fn path_tooltip(label: &'static str) -> Element<'static, Message> {
    container(readable_text(label).size(11))
        .padding([4, 6])
        .into()
}

fn content_indexing_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    vec![
        toggle_setting_row(
            "Index File Contents",
            None,
            browser.user_config().search_content_indexing_enabled,
            Message::SearchContentIndexingToggled,
        ),
        info_setting_row(
            localized_text(format!(
                "Maximum content extraction: {}",
                format_file_size(browser.user_config().search_max_extract_bytes)
            ))
            .size(12)
            .into(),
        ),
    ]
}

fn status_overview_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let service = &browser.search_service;
    let mut rows = Vec::new();

    match &service.endpoint {
        SearchEndpointState::Starting => rows.push(status_notice(
            "Checking the search service. Status will update automatically.",
        )),
        SearchEndpointState::Connected => {}
        SearchEndpointState::Rechecking {
            consecutive_failures,
            ..
        } => rows.push(status_notice(format!(
            "Connection is unstable. Rechecking automatically ({consecutive_failures}/3); showing the last confirmed status."
        ))),
        SearchEndpointState::Unavailable { diagnostic } => {
            rows.push(diagnostic_notice(diagnostic.kind));
        }
    }

    if let Some(status) = service.confirmed_status.as_ref() {
        rows.extend(confirmed_status_summary_rows(status));
        if let Some(confirmed_at) = service.last_confirmed_at {
            rows.push(info_setting_row(
                localized_text(format!(
                    "Last confirmed: {}",
                    format_system_time(confirmed_at)
                ))
                .size(11)
                .into(),
            ));
        }
    } else {
        rows.extend([
            summary_value_row(
                "Service",
                endpoint_without_snapshot_label(&service.endpoint),
            ),
            summary_value_row("Indexed search", "Waiting for service status"),
            summary_value_row("Index maintenance", "Not initialized"),
        ]);
    }

    rows
}

fn confirmed_status_summary_rows(status: &SearchServiceStatus) -> Vec<Element<'static, Message>> {
    vec![
        summary_value_row("Service", service_phase_label(&status.phase)),
        summary_value_row(
            "Indexed search",
            query_availability_label(&status.query_availability),
        ),
        summary_value_row("Index maintenance", index_maintenance_label(status)),
    ]
}

fn service_phase_label(phase: &SearchServicePhase) -> &'static str {
    match phase {
        SearchServicePhase::Starting => "Starting",
        SearchServicePhase::Ready => "Running",
        SearchServicePhase::Degraded { .. } => "Needs attention",
        SearchServicePhase::Failed { .. } => "Unavailable",
        SearchServicePhase::ShuttingDown => "Stopping",
    }
}

fn query_availability_label(availability: &IndexedQueryAvailability) -> &'static str {
    match availability {
        IndexedQueryAvailability::Available => "Available",
        IndexedQueryAvailability::Unavailable { .. } => "Temporarily unavailable",
    }
}

fn index_maintenance_label(status: &SearchServiceStatus) -> &'static str {
    let Some(index_status) = status.index_status.as_ref() else {
        return "Not initialized";
    };
    match index_status.health {
        IndexHealth::Healthy => "Healthy",
        IndexHealth::Degraded { .. } => "Needs attention",
        IndexHealth::Error { .. } => "Unavailable",
    }
}

fn endpoint_without_snapshot_label(endpoint: &SearchEndpointState) -> &'static str {
    match endpoint {
        SearchEndpointState::Starting => "Checking",
        SearchEndpointState::Connected => "Running",
        SearchEndpointState::Rechecking { .. } => "Rechecking",
        SearchEndpointState::Unavailable { .. } => "Unavailable",
    }
}

fn diagnostic_notice(kind: SearchServiceDiagnosticKind) -> Element<'static, Message> {
    info_setting_row(
        column![
            localized_text(diagnostic_title(kind)).size(13),
            localized_text(diagnostic_recommendation(kind))
                .size(11)
                .width(Length::Fill),
        ]
        .spacing(3)
        .width(Length::Fill)
        .into(),
    )
}

fn status_notice(message: impl Into<String>) -> Element<'static, Message> {
    info_setting_row(
        localized_text(message.into())
            .size(11)
            .width(Length::Fill)
            .into(),
    )
}

fn summary_value_row(label: &'static str, value: impl Into<String>) -> Element<'static, Message> {
    labeled_setting_row(
        label,
        localized_text(value.into())
            .size(12)
            .width(Length::Shrink)
            .into(),
    )
}

fn index_progress_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    let Some(status) = browser.search_service.confirmed_status.as_ref() else {
        return vec![status_notice(
            "Index progress will appear after the service responds.",
        )];
    };
    let Some(index_status) = status.index_status.as_ref() else {
        return vec![status_notice("The index has not been initialized yet.")];
    };

    let mut rows = vec![
        summary_value_row("Index phase", index_phase_label(&index_status.phase)),
        summary_value_row(
            "Indexed items",
            format_count(index_status.visible_indexed_files),
        ),
    ];
    match &index_status.phase {
        IndexPhase::Checking {
            checked_entries,
            changed_entries,
        } => {
            rows.push(summary_value_row(
                "Checked items",
                format_count(*checked_entries),
            ));
            rows.push(summary_value_row(
                "Changed items",
                format_count(*changed_entries),
            ));
        }
        IndexPhase::Crawling {
            scanned_entries,
            current_scope,
        } => {
            rows.push(summary_value_row(
                "Scanned items",
                format_count(*scanned_entries),
            ));
            rows.push(summary_value_row(
                "Current location",
                current_scope.to_string_lossy().into_owned(),
            ));
        }
        IndexPhase::Applying { pending_mutations } => rows.push(summary_value_row(
            "Pending changes",
            format_count(*pending_mutations),
        )),
        IndexPhase::Starting | IndexPhase::Complete | IndexPhase::Failed { .. } => {}
    }
    rows
}

fn index_phase_label(phase: &IndexPhase) -> &'static str {
    match phase {
        IndexPhase::Starting => "Starting",
        IndexPhase::Checking { .. } => "Checking existing files",
        IndexPhase::Crawling { .. } => "Scanning files",
        IndexPhase::Applying { .. } => "Applying changes",
        IndexPhase::Complete => "Up to date",
        IndexPhase::Failed { .. } => "Failed",
    }
}

fn recent_issue_rows(browser: &FileBrowser) -> Vec<Element<'_, Message>> {
    if browser.search_service.incidents.is_empty() {
        return vec![status_notice(
            "No search service issues detected during this app session.",
        )];
    }

    browser
        .search_service
        .incidents
        .iter()
        .map(issue_row)
        .collect()
}

fn issue_row(incident: &SearchServiceIncident) -> Element<'_, Message> {
    let state_label = match incident.state {
        SearchServiceIncidentState::Current => "Current issue",
        SearchServiceIncidentState::Recovered => "Recovered",
    };
    let occurrence_label = if incident.occurrence_count == 1 {
        "Occurred once".to_owned()
    } else {
        format!("Occurred {} times", incident.occurrence_count)
    };
    let metadata = format!(
        "{} · {} · {}: {} · {}: {}",
        crate::localization::translate_current(state_label),
        crate::localization::translate_current(&occurrence_label),
        crate::localization::translate_current("First"),
        format_system_time(incident.first_seen),
        crate::localization::translate_current("Latest"),
        format_system_time(incident.last_seen),
    );
    let detail_button_label = if incident.technical_detail_expanded {
        "Hide technical details"
    } else {
        "Show technical details"
    };
    let detail_button =
        button(container(localized_text(detail_button_label).size(11)).padding([4, 8]))
            .on_press(Message::SearchServiceIncidentDetailsToggled(incident.kind))
            .style(context_menu_button_style());

    let mut content = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(localized_text(diagnostic_title(incident.kind)).size(13))
        .push(localized_text(diagnostic_recommendation(incident.kind)).size(11))
        .push(readable_text(metadata).size(10))
        .push(detail_button);

    if incident.technical_detail_expanded {
        let copy_button =
            button(container(localized_text("Copy technical details").size(11)).padding([4, 8]))
                .on_press(Message::SearchServiceIncidentDetailsCopyRequested(
                    incident.kind,
                ))
                .style(context_menu_button_style());
        content = content
            .push(
                readable_text(&incident.technical_detail)
                    .size(10)
                    .width(Length::Fill),
            )
            .push(copy_button);
    }

    info_setting_row(content.into())
}

fn service_recovery_rows(browser: &FileBrowser) -> Vec<Element<'static, Message>> {
    let recovery = &browser.search_service.recovery;
    let recovery_is_running = recovery.is_running();
    let restart_row = action_setting_row(
        "Restart Index Service",
        "Gracefully stop and restart the managed service.",
    );
    let restart_row = if recovery_is_running {
        restart_row
    } else {
        restart_row.on_press(Message::SearchServiceRestartRequested)
    };

    let force_restart_row = if *recovery == SearchServiceRecoveryState::ConfirmingForceRestart {
        action_setting_row(
            "Click Again to Force Restart",
            "Current indexing work will stop. Index data and settings will be kept.",
        )
        .style(destructive_confirmation_button_style())
    } else {
        action_setting_row(
            "Force Restart",
            "Use only when the managed service is unresponsive.",
        )
    };
    let force_restart_row = if recovery_is_running {
        force_restart_row
    } else {
        force_restart_row.on_press(Message::SearchServiceForceRestartPressed)
    };

    let mut rows: Vec<Element<'static, Message>> =
        vec![restart_row.into(), force_restart_row.into()];
    if let Some(status_text) = recovery_status_text(recovery) {
        rows.push(status_notice(status_text));
    }
    rows
}

fn recovery_status_text(state: &SearchServiceRecoveryState) -> Option<&'static str> {
    match state {
        SearchServiceRecoveryState::Idle | SearchServiceRecoveryState::ConfirmingForceRestart => {
            None
        }
        SearchServiceRecoveryState::Running(SearchServiceRecoveryAction::Restart) => {
            Some("Restarting index service...")
        }
        SearchServiceRecoveryState::Running(SearchServiceRecoveryAction::ForceRestart) => {
            Some("Force restarting index service...")
        }
        SearchServiceRecoveryState::Succeeded(SearchServiceRecoveryAction::Restart) => {
            Some("Index service restarted successfully.")
        }
        SearchServiceRecoveryState::Succeeded(SearchServiceRecoveryAction::ForceRestart) => {
            Some("Index service force restarted successfully.")
        }
        SearchServiceRecoveryState::Failed { .. } => {
            Some("Index service restart failed. See Recent Issues for technical details.")
        }
    }
}

fn diagnostic_title(kind: SearchServiceDiagnosticKind) -> &'static str {
    match kind {
        SearchServiceDiagnosticKind::EndpointTimedOut => "Search service response is delayed",
        SearchServiceDiagnosticKind::EndpointUnavailable => "Cannot reach the search service",
        SearchServiceDiagnosticKind::ServiceChanging => "Search service is changing",
        SearchServiceDiagnosticKind::ServiceUnverified => "Cannot verify the search service",
        SearchServiceDiagnosticKind::EndpointInvalid => "Search service connection is inconsistent",
        SearchServiceDiagnosticKind::ComponentIncompatible => {
            "Search service components do not match"
        }
        SearchServiceDiagnosticKind::RecoveryFailed => "Could not restart the search service",
        SearchServiceDiagnosticKind::ServiceDegraded => "Search service needs attention",
        SearchServiceDiagnosticKind::ServiceFailed => "Search service stopped working",
        SearchServiceDiagnosticKind::IndexedQueriesUnavailable => {
            "Indexed search is temporarily unavailable"
        }
        SearchServiceDiagnosticKind::IndexFailed => "Index update failed",
        SearchServiceDiagnosticKind::IndexMaintenanceDegraded => {
            "Index maintenance needs attention"
        }
        SearchServiceDiagnosticKind::IndexMaintenanceFailed => "Index maintenance stopped",
    }
}

fn diagnostic_recommendation(kind: SearchServiceDiagnosticKind) -> &'static str {
    match kind {
        SearchServiceDiagnosticKind::EndpointTimedOut
        | SearchServiceDiagnosticKind::EndpointUnavailable
        | SearchServiceDiagnosticKind::ServiceChanging => {
            "Wait for automatic rechecking. If the problem continues, restart the index service."
        }
        SearchServiceDiagnosticKind::ServiceUnverified => {
            "Review the technical details. Restart the index service if the problem continues."
        }
        SearchServiceDiagnosticKind::EndpointInvalid => {
            "Restart the index service to establish a verified connection."
        }
        SearchServiceDiagnosticKind::ComponentIncompatible => {
            "Reinstall the search components from the current Bennu package."
        }
        SearchServiceDiagnosticKind::RecoveryFailed => {
            "Review the technical details. Use force restart only if the service remains unresponsive."
        }
        SearchServiceDiagnosticKind::ServiceDegraded
        | SearchServiceDiagnosticKind::IndexedQueriesUnavailable
        | SearchServiceDiagnosticKind::IndexMaintenanceDegraded => {
            "Search may use a slower fallback while the service recovers."
        }
        SearchServiceDiagnosticKind::ServiceFailed
        | SearchServiceDiagnosticKind::IndexFailed
        | SearchServiceDiagnosticKind::IndexMaintenanceFailed => {
            "Restart the index service. Existing index data and settings will be kept."
        }
    }
}

fn format_count(value: u64) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(*byte as char);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use file_search::{IndexedQueryAvailability, SearchServicePhase, SearchServiceStatus};

    use super::*;
    use crate::config;
    use crate::model::SearchServiceState;

    #[test]
    fn diagnostic_copy_uses_stable_user_facing_text() {
        assert_eq!(
            diagnostic_title(SearchServiceDiagnosticKind::EndpointTimedOut),
            "Search service response is delayed"
        );
        assert!(
            diagnostic_recommendation(SearchServiceDiagnosticKind::ComponentIncompatible)
                .contains("Reinstall")
        );
    }

    #[test]
    fn count_formatting_groups_large_index_totals() {
        assert_eq!(format_count(98_765), "98,765");
    }

    #[test]
    fn routine_background_refresh_does_not_change_overview_row_count() {
        let (mut browser, _) = FileBrowser::new(config::default_user_config());
        browser.search_service = SearchServiceState::new();
        let initial_request = browser.search_service.begin_initial_status_request();
        browser.search_service.accept_status_request(
            initial_request,
            Ok(SearchServiceStatus {
                phase: SearchServicePhase::Ready,
                query_availability: IndexedQueryAvailability::Available,
                index_status: None,
            }),
        );
        let settled_row_count = status_overview_rows(&browser).len();

        let refresh_request = browser.search_service.request_status_refresh();

        assert!(refresh_request.is_some());
        assert_eq!(status_overview_rows(&browser).len(), settled_row_count);
    }
}
