use super::*;
use relm4::{Component, ComponentController};
use std::path::Path;

fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args([
            "-c",
            "user.name=Commander Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn initialize(directory: &Path, branch: &str) {
    std::fs::create_dir_all(directory).unwrap();
    git(directory, &["init", "-b", branch]);
    std::fs::write(directory.join("notes.txt"), "initial\n").unwrap();
    git(directory, &["add", "notes.txt"]);
    git(directory, &["commit", "-m", "Initial notes"]);
}

#[test]
fn repository_status_handles_nested_paths_changes_tracking_and_worktrees() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    initialize(&root, "main");
    let cancel = CancelToken::new();
    let status = || read_status(&VPath::from(root.as_path()), &cancel).unwrap();
    assert_eq!(status().branch_label(), "main");
    assert!(!status().changed);
    let index = std::fs::read(root.join(".git/index")).unwrap();
    std::fs::write(root.join("notes.txt"), "edited\n").unwrap();
    assert!(status().changed);
    assert_eq!(
        std::fs::read(root.join(".git/index")).unwrap(),
        index,
        "background status must not write or lock the index"
    );
    git(&root, &["add", "notes.txt"]);
    assert!(status().changed, "staged changes count too");
    git(&root, &["commit", "-m", "Edit notes"]);
    git(&root, &["branch", "upstream", "HEAD~1"]);
    git(&root, &["branch", "--set-upstream-to=upstream"]);
    assert_eq!((status().ahead, status().behind), (1, 0));
    assert!(status().tooltip().contains("Tracking upstream"));
    git(&root, &["branch", "-f", "upstream", "HEAD"]);
    git(&root, &["checkout", "--detach", "HEAD~1"]);
    assert!(status().branch_label().starts_with("HEAD · "));
    git(&root, &["checkout", "main"]);

    let child = root.join("empty-child");
    std::fs::create_dir(&child).unwrap();
    assert_eq!(
        read_status(&VPath::from(child.as_path()), &cancel)
            .unwrap()
            .branch,
        "main"
    );
    std::fs::write(root.join("untracked\n# branch.head impostor"), "untracked").unwrap();
    assert!(status().changed);
    assert_eq!(status().branch, "main");
    let worktree = fixture.path().join("worktree");
    git(
        &root,
        &["worktree", "add", "-b", "topic", worktree.to_str().unwrap()],
    );
    let info = read_status(&VPath::from(worktree.as_path()), &cancel).unwrap();
    assert_eq!(info.branch, "topic");
    assert!(!info.changed);
    assert!(read_status(&VPath::from(fixture.path()), &cancel).is_none());

    let empty = fixture.path().join("new-repository");
    std::fs::create_dir(&empty).unwrap();
    git(&empty, &["init", "-b", "new-project"]);
    let info = read_status(&VPath::from(empty.as_path()), &cancel).unwrap();
    assert_eq!(info.branch_label(), "new-project");
    assert!(info.tooltip().contains("No commits yet"));
}

#[test]
fn porcelain_headers_and_renames_cannot_be_confused_with_filenames() {
    let output = b"# branch.oid abcdef123456789\0# branch.head topic\0# branch.upstream origin/topic\0# branch.ab +2 -3\0# future.header ignored\0\
        2 R. N... 100644 100644 100644 a b R100 renamed\0# branch.head not-a-branch\0\
        u UU N... 100644 100644 100644 100644 a b c conflict\0? untracked\0";
    let info = parse_status(&output[..]).unwrap();
    assert_eq!(info.branch, "topic");
    assert_eq!(info.summary(), "! ↑2 ↓3");
    assert!(info.tooltip().contains("merge conflicts"));
    let clean = parse_status(&b"# branch.oid abcdef123\0# branch.head main\0"[..]).unwrap();
    assert!(clean.summary().is_empty());
    assert!(parse_status(&b"fatal: not a git repository\0"[..]).is_none());
}

#[test]
fn unavailable_slow_and_cancelled_queries_are_silent_and_stale_results_are_discarded() {
    let cancel = CancelToken::new();
    assert!(
        query(
            Command::new("/nonexistent/commander-test-git"),
            &cancel,
            QUERY_TIMEOUT
        )
        .is_none()
    );
    let mut slow = Command::new("/bin/sleep");
    slow.arg("10");
    let started = Instant::now();
    assert!(query(slow, &cancel, Duration::from_millis(40)).is_none());
    assert!(started.elapsed() < Duration::from_secs(2));
    cancel.cancel();
    assert!(query(Command::new("git"), &cancel, QUERY_TIMEOUT).is_none());

    let mut state = PaneGitState::default();
    let old = VPath::from("/old");
    state.prepare(Some(old.clone()));
    let cancel = CancelToken::new();
    state.inflight = Some((old.clone(), cancel.clone()));
    state.prepare(Some(VPath::from("/new")));
    assert!(cancel.is_cancelled());
    // Returning to the old path still must not accept its cancelled query.
    state.prepare(Some(old.clone()));
    state.finish(
        &old,
        Some(GitStatus {
            branch: "stale".into(),
            ..GitStatus::default()
        }),
    );
    assert!(state.info.is_none());
    assert!(state.inflight.is_none());
}

#[track_caller]
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(
            Instant::now() < deadline,
            "Git footer timed out at {}",
            std::panic::Location::caller()
        );
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
}

fn snapshot(window: &adw::ApplicationWindow, name: &str) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        glib::MainContext::default().iteration(false);
        thread::sleep(Duration::from_millis(2));
    }
    let Some(directory) = std::env::var_os("COMMANDER_TEST_ARTIFACTS") else {
        return;
    };
    let child = gtk::prelude::GtkWindowExt::child(window).unwrap();
    let snapshot = gtk::Snapshot::new();
    window.snapshot_child(&child, &snapshot);
    window
        .renderer()
        .unwrap()
        .render_texture(snapshot.to_node().unwrap(), None)
        .save_to_png(
            Path::new(&directory)
                .join("snapshots")
                .join(format!("{name}.png")),
        )
        .unwrap();
}

#[test]
#[ignore = "requires an isolated GTK session; run in the native suite"]
fn gtk_pane_git_tracks_each_directory_and_fits_quietly_in_the_footer() {
    assert_eq!(std::env::var("COMMANDER_ISOLATED_TEST").as_deref(), Ok("1"));
    adw::init().unwrap();
    relm4::main_adw_application()
        .register(gio::Cancellable::NONE)
        .unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let left = fixture.path().join("Commander");
    let right = fixture.path().join("Design-library");
    initialize(&left, "main");
    initialize(&right, "feature/refine-navigation");
    std::fs::write(left.join("notes.txt"), "changes\n").unwrap();
    let (session_worker, startup) = SessionWorker::start().unwrap();
    let mut session = SessionState {
        dual_pane: true,
        sidebar_visible: false,
        preview_visible: false,
        window_width: 1320,
        window_height: 720,
        ..SessionState::default()
    };
    session.left.view_mode = PaneViewMode::List;
    session.right.view_mode = PaneViewMode::List;
    let app = AppModel::builder()
        .launch(AppInit {
            options: AppOptions {
                left: Some(VPath::from(left.as_path())),
                right: Some(VPath::from(right.as_path())),
                ..AppOptions::default()
            },
            session_worker,
            session: Some(session),
            keymap_overrides: startup.keymap_overrides,
            history: startup.history,
            history_warning: startup.history_warning,
            started: Instant::now(),
        })
        .detach();
    app.widget().present();
    wait_until(|| {
        app.widgets().panes[0].git.root.is_visible() && app.widgets().panes[1].git.root.is_visible()
    });
    assert_eq!(app.widgets().panes[0].git.branch.text(), "main");
    assert_eq!(app.widgets().panes[0].git.summary.text(), "*");
    assert_eq!(
        app.widgets().panes[1].git.branch.text(),
        "feature/refine-navigation"
    );
    assert!(!app.widgets().panes[1].git.summary.is_visible());
    for (appearance, name) in [
        (AppearanceMode::Dark, "git-footer-dark"),
        (AppearanceMode::Light, "git-footer-light"),
    ] {
        apply_appearance(appearance);
        snapshot(app.widget(), name);
    }

    // A branch switch outside Commander refreshes even though the folder's files are unchanged.
    git(
        &left,
        &[
            "switch",
            "-c",
            "feature/a-very-long-branch-name-for-narrow-file-panels",
        ],
    );
    wait_until(|| {
        app.widgets().panes[0]
            .git
            .branch
            .text()
            .starts_with("feature/a-very-long")
    });
    assert_eq!(
        app.widgets().panes[1].git.branch.text(),
        "feature/refine-navigation"
    );
    app.widget().set_default_size(720, 680);
    // GTK may clamp to the existing toolbar's minimum width.
    wait_until(|| app.widget().width() < 1000);
    apply_appearance(AppearanceMode::Dark);
    snapshot(app.widget(), "git-footer-narrow-dark");
    apply_appearance(AppearanceMode::Light);
    snapshot(app.widget(), "git-footer-narrow-light");
    let widgets = app.widgets();
    for pane in &widgets.panes {
        assert!(pane.git.root.width() < 290, "Git context must stay compact");
        assert!(
            pane.status.width() > 120,
            "item/selection status keeps room"
        );
    }
    drop(widgets);

    app.emit(AppMsg::NavigateExact(
        PaneId::Left,
        VPath::from(fixture.path()),
    ));
    wait_until(|| !app.widgets().panes[0].git.root.is_visible());
    assert!(app.widgets().panes[1].git.root.is_visible());
    app.emit(AppMsg::NavigateExact(
        PaneId::Left,
        VPath::from(left.as_path()),
    ));
    wait_until(|| app.widgets().panes[0].git.root.is_visible());
    app.emit(AppMsg::NewTab(PaneId::Left));
    app.emit(AppMsg::NavigateExact(
        PaneId::Left,
        VPath::from(right.as_path()),
    ));
    wait_until(|| app.widgets().panes[0].git.branch.text() == "feature/refine-navigation");
    app.emit(AppMsg::SelectTab(PaneId::Left, 0));
    wait_until(|| {
        app.widgets().panes[0]
            .git
            .branch
            .text()
            .starts_with("feature/a-very-long")
    });
    app.widget().close();
}
