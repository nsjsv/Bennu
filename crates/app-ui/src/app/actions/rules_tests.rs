use super::*;
use crate::actions::type_filter::TypeFilter;
use crate::actions::{
    ActionDefinition, ActionLibrary, ActionNotice, ActionStep, ActionWatch, PathTemplate,
};
use crate::app::task_driving_tests::drive;
use crate::model::BrowserLocation;

fn rule(folder: &Path) -> ActionDefinition {
    ActionDefinition {
        id: ActionId::from_file_stem("tidy"),
        name: "整理图片".to_owned(),
        file: "/actions/tidy.toml".into(),
        types: TypeFilter::Anything,
        steps: vec![ActionStep::MoveTo(
            PathTemplate::parse("{dir}/整理").unwrap(),
        )],
        notice: ActionNotice::Silent,
        watch: Some(ActionWatch {
            query: "类型:图片".to_owned(),
            folder: folder.to_path_buf(),
        }),
    }
}

fn browser_with_rule(folder: &Path) -> FileBrowser {
    let (mut browser, _) = FileBrowser::new(crate::config::default_user_config());
    browser.tab_mut().location = BrowserLocation::Directory(std::env::temp_dir());
    browser.system_focused_window = Some(iced::window::Id::unique());
    browser.action_library = ActionLibrary::from_definitions(vec![rule(folder)]);
    browser
}

fn tidy() -> ActionId {
    ActionId::from_file_stem("tidy")
}

fn settled(path: &Path) -> SettledFile {
    let metadata = std::fs::metadata(path).unwrap();
    SettledFile {
        size: metadata.len(),
        modified: metadata.modified().ok(),
        accessed: metadata.accessed().ok(),
        created: metadata.created().ok(),
    }
}

#[test]
fn the_query_is_matched_by_name_and_type() {
    let file = SettledFile {
        size: 10,
        modified: Some(SystemTime::now()),
        accessed: None,
        created: None,
    };
    assert!(matches_query("类型:图片", Path::new("/w/a.jpg"), file));
    assert!(!matches_query("类型:图片", Path::new("/w/a.txt"), file));
    assert!(matches_query("截图", Path::new("/w/截图 2026.png"), file));
    assert!(!matches_query("截图", Path::new("/w/照片.png"), file));
    assert!(
        matches_query("", Path::new("/w/anything"), file),
        "nothing is everything"
    );
    assert!(!matches_query("大小:>1m", Path::new("/w/a.jpg"), file));
}

#[tokio::test]
async fn a_finished_matching_file_is_handled_once_and_the_result_is_not_handled_again() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().canonicalize().unwrap();
    let mut browser = browser_with_rule(&folder);
    let photo = folder.join("a.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    std::fs::write(folder.join("a.txt"), b"text").unwrap();

    let ran = browser.accept_rule_file_settled(tidy(), photo.clone(), settled(&photo));
    drive(&mut browser, ran).await;
    let moved = folder.join("整理/a.jpg");
    assert_eq!(std::fs::read(&moved).unwrap(), b"jpeg");

    // 同一个文件再来一次（比如又改了一下）：不再跑。
    std::fs::write(&photo, b"again").unwrap();
    let again = browser.accept_rule_file_settled(tidy(), photo.clone(), settled(&photo));
    drive(&mut browser, again).await;
    assert!(photo.exists(), "handled once");
    // 跑出来的文件记成处理过。
    assert!(browser.rules.handled.contains(&(tidy(), moved)));

    let text = folder.join("a.txt");
    let other = browser.accept_rule_file_settled(tidy(), text.clone(), settled(&text));
    drive(&mut browser, other).await;
    assert!(text.exists(), "not an image");
}

#[tokio::test]
async fn a_file_is_handled_only_after_it_stops_changing() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().canonicalize().unwrap();
    let mut browser = browser_with_rule(&folder);
    let writing = folder.join("下载中.png");
    std::fs::write(&writing, b"p").unwrap();
    let appending = writing.clone();
    let writer = std::thread::spawn(move || {
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(250));
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&appending)
                .unwrap();
            std::io::Write::write_all(&mut file, b"more").unwrap();
        }
    });
    drive(&mut browser, settle(tidy(), writing.clone())).await;
    writer.join().unwrap();
    assert!(writing.exists(), "it was still being written");

    drive(&mut browser, settle(tidy(), writing.clone())).await;
    assert!(
        folder.join("整理/下载中.png").exists(),
        "handled once it settled"
    );
}

#[test]
fn a_rule_stops_after_too_many_files_in_a_minute() {
    let mut rules = Rules::default();
    let now = Instant::now();
    for _ in 0..MOST_PER_MINUTE {
        assert!(rules.take_turn(&tidy(), now));
    }
    assert!(!rules.take_turn(&tidy(), now));
    assert!(
        rules.take_turn(&tidy(), now + MINUTE),
        "a minute later there is room again"
    );
}
