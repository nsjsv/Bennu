use std::path::PathBuf;

use desktop_linux::OpenError;

#[test]
fn open_error_failed_keeps_path() {
    let path = PathBuf::from("/tmp/example");
    let error = OpenError::Spawn {
        path: path.clone(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
    };

    match error {
        OpenError::Spawn { path: actual, .. } => assert_eq!(actual, path),
        _ => panic!("unexpected variant"),
    }
}
