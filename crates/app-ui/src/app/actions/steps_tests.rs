use std::path::Path;

use super::*;
use crate::actions::template::CommandTemplate;

fn context(working_directory: &Path, selected: &[PathBuf]) -> StepContext {
    StepContext {
        selected: selected.to_vec(),
        values: RunValues {
            date: "2026-11-19".to_owned(),
            home: PathBuf::from("/home/u"),
        },
        working_directory: working_directory.to_path_buf(),
    }
}

async fn prepare(step: ActionStep, input: &[PathBuf], working_directory: &Path) -> PreparedStep {
    prepare_step(step, input.to_vec(), context(working_directory, input)).await
}

fn ready(prepared: PreparedStep) -> StepPlan {
    match prepared {
        PreparedStep::Ready(plan) => plan,
        PreparedStep::Failed(reason) => panic!("the step failed: {reason}"),
    }
}

fn operations(plan: &StepPlan) -> Vec<&QueuedFileOperation> {
    plan.work.iter().map(|work| &work.operation).collect()
}

#[tokio::test]
async fn moving_into_an_existing_folder_keeps_both_on_a_clash() {
    let directory = tempfile::tempdir().unwrap();
    let music = directory.path().join("music");
    std::fs::create_dir(&music).unwrap();
    let song = directory.path().join("a.m4a");
    let already_there = music.join("b.m4a");
    let plan = ready(
        prepare(
            ActionStep::MoveTo(PathTemplate::parse(music.to_str().unwrap()).unwrap()),
            &[song.clone(), already_there.clone()],
            directory.path(),
        )
        .await,
    );

    assert_eq!(plan.passed_through, [already_there]);
    let [QueuedFileOperation::Move { transfers, .. }] = operations(&plan)[..] else {
        panic!("expected one move: {plan:?}");
    };
    assert_eq!(transfers.len(), 1);
    assert_eq!(transfers[0].source, song);
    assert_eq!(transfers[0].target, music.join("a.m4a"));
    assert_eq!(
        transfers[0].conflict_strategy,
        TransferConflictStrategy::KeepBoth
    );
}

#[tokio::test]
async fn missing_folders_are_made_in_the_queue() {
    let directory = tempfile::tempdir().unwrap();
    let video = directory.path().join("v.mp4");

    let one_level = ready(
        prepare(
            ActionStep::MoveTo(PathTemplate::parse("{dir}/音频").unwrap()),
            std::slice::from_ref(&video),
            directory.path(),
        )
        .await,
    );
    assert!(matches!(
        operations(&one_level)[..],
        [QueuedFileOperation::GatherSelectionIntoNewFolder { directory: target, sources }]
            if *target == directory.path().join("音频") && *sources == [video.clone()]
    ));

    let nested = ready(
        prepare(
            ActionStep::MoveTo(PathTemplate::parse("{dir}/a/b").unwrap()),
            std::slice::from_ref(&video),
            directory.path(),
        )
        .await,
    );
    assert!(matches!(
        operations(&nested)[..],
        [QueuedFileOperation::CreateDirectories { parent, names, gather_sources }]
            if parent == directory.path() && *names == ["a/b"] && *gather_sources == [video.clone()]
    ));

    let copied = ready(
        prepare(
            ActionStep::CopyTo(PathTemplate::parse("{dir}/backup").unwrap()),
            std::slice::from_ref(&video),
            directory.path(),
        )
        .await,
    );
    assert!(matches!(
        &copied.work[..],
        [StepWork {
            operation: QueuedFileOperation::CreateDirectories { gather_sources, .. },
            then: Some(FollowUp::CopyInto(sources)),
        }] if gather_sources.is_empty() && *sources == [video.clone()]
    ));
}

#[tokio::test]
async fn a_file_where_the_folder_should_be_fails_the_step() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("音频"), b"").unwrap();
    let prepared = prepare(
        ActionStep::MoveTo(PathTemplate::parse("{dir}/音频").unwrap()),
        &[directory.path().join("v.mp4")],
        directory.path(),
    )
    .await;
    assert!(matches!(prepared, PreparedStep::Failed(_)));
}

#[tokio::test]
async fn archives_get_a_free_name_beside_the_sources() {
    let directory = tempfile::tempdir().unwrap();
    let report = directory.path().join("report.txt");
    std::fs::write(&report, b"x").unwrap();
    std::fs::write(directory.path().join("report.zip"), b"taken").unwrap();

    let plan = ready(
        prepare(
            ActionStep::Compress(ArchiveFormat::Zip),
            std::slice::from_ref(&report),
            directory.path(),
        )
        .await,
    );
    assert!(matches!(
        operations(&plan)[..],
        [QueuedFileOperation::CreateArchive { target, password: None, .. }]
            if *target == directory.path().join("report 2.zip")
    ));
}

#[tokio::test]
async fn extracting_takes_only_archives() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("photos.zip");
    let notes = directory.path().join("notes.txt");
    let plan = ready(
        prepare(
            ActionStep::Extract(file_core::ArchiveExtractionPlacement::Smart),
            &[archive.clone(), notes],
            directory.path(),
        )
        .await,
    );
    assert!(matches!(
        operations(&plan)[..],
        [QueuedFileOperation::ExtractArchive { request }] if request.archive == archive
    ));
    assert_eq!(plan.skipped.len(), 1);

    let nothing = prepare(
        ActionStep::Extract(file_core::ArchiveExtractionPlacement::Smart),
        &[directory.path().join("notes.txt")],
        directory.path(),
    )
    .await;
    assert!(matches!(nothing, PreparedStep::Failed(_)));
}

#[tokio::test]
async fn commands_run_in_the_working_folder_and_pass_the_items_on() {
    let directory = tempfile::tempdir().unwrap();
    let items = [directory.path().join("a b.mp4")];
    let plan = ready(
        prepare(
            ActionStep::Run(CommandTemplate::parse("ffprobe {file}").unwrap()),
            &items,
            directory.path(),
        )
        .await,
    );
    assert_eq!(plan.passed_through, items);
    let [QueuedFileOperation::RunCommands { invocations }] = operations(&plan)[..] else {
        panic!("expected commands: {plan:?}");
    };
    assert_eq!(invocations[0].program, "ffprobe");
    assert_eq!(invocations[0].arguments, [items[0].as_os_str()]);
    assert_eq!(invocations[0].working_directory, directory.path());

    // 一次的命令不需要输入。
    let once = ready(
        prepare(
            ActionStep::Run(CommandTemplate::parse("git pull").unwrap()),
            &[],
            directory.path(),
        )
        .await,
    );
    assert_eq!(operations(&once).len(), 1);
}

#[tokio::test]
async fn trash_takes_the_current_batch_or_the_original_selection() {
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("photos.zip");
    let extracted = directory.path().join("photos");
    let selected = prepare_step(
        ActionStep::Trash(TrashScope::Selected),
        vec![extracted.clone()],
        context(directory.path(), std::slice::from_ref(&archive)),
    )
    .await;
    assert!(matches!(
        operations(&ready(selected))[..],
        [QueuedFileOperation::Trash { paths }] if *paths == [archive.clone()]
    ));
    let current = prepare(
        ActionStep::Trash(TrashScope::Current),
        &[extracted.clone()],
        directory.path(),
    )
    .await;
    assert!(matches!(
        operations(&ready(current))[..],
        [QueuedFileOperation::Trash { paths }] if *paths == [extracted.clone()]
    ));
}

#[tokio::test]
async fn steps_with_nothing_to_work_on_fail() {
    let directory = tempfile::tempdir().unwrap();
    for step in [
        ActionStep::Compress(ArchiveFormat::TarGz),
        ActionStep::Trash(TrashScope::Current),
        ActionStep::Convert(ConvertFormat::named("m4a").unwrap()),
    ] {
        assert!(matches!(
            prepare(step, &[], directory.path()).await,
            PreparedStep::Failed(_)
        ));
    }
    let notes = directory.path().join("notes.txt");
    std::fs::write(&notes, b"text").unwrap();
    assert!(matches!(
        prepare(
            ActionStep::Convert(ConvertFormat::named("m4a").unwrap()),
            &[notes],
            directory.path()
        )
        .await,
        PreparedStep::Failed(_)
    ));
}
