use std::fs;
use std::path::PathBuf;
use std::process::Command;

use tempfile::TempDir;

use super::*;

mod operations;
mod paths;
mod refresh;
mod refs_and_topology;
mod snapshots;

struct TestRepo {
    temp: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let repo = Self { temp };
        repo.git(["init", "--initial-branch=main"]);
        repo.git(["config", "user.email", "tester@example.com"]);
        repo.git(["config", "user.name", "Tester"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        repo.git(["config", "tag.gpgsign", "false"]);
        repo
    }

    fn clone_from(remote: &Path) -> Self {
        let temp = TempDir::new().unwrap();
        let output = Command::new("git")
            .arg("clone")
            .arg(remote)
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git clone failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let repo = Self { temp };
        repo.git(["config", "user.email", "tester@example.com"]);
        repo.git(["config", "user.name", "Tester"]);
        repo.git(["config", "commit.gpgsign", "false"]);
        repo.git(["config", "tag.gpgsign", "false"]);
        repo
    }

    fn path(&self) -> &Path {
        self.temp.path()
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    fn current_branch(&self) -> String {
        self.git_stdout(["symbolic-ref", "--short", "HEAD"])
    }

    fn git<const N: usize>(&self, args: [&str; N]) {
        let output = self.git_output(args);
        assert_git_success(output);
    }

    fn git_allow_file_protocol<const N: usize>(&self, args: [&str; N]) {
        let output = Command::new("git")
            .arg("-c")
            .arg("protocol.file.allow=always")
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap();
        assert_git_success(output);
    }

    fn git_expect_failure<const N: usize>(&self, args: [&str; N]) {
        let output = self.git_output(args);
        assert!(
            !output.status.success(),
            "git command unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output<const N: usize>(&self, args: [&str; N]) -> std::process::Output {
        Command::new("git")
            .args(args)
            .current_dir(self.path())
            .output()
            .unwrap()
    }

    fn git_stdout<const N: usize>(&self, args: [&str; N]) -> String {
        let output = self.git_output(args);
        assert_git_success(output)
    }
}

fn assert_git_success(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "git command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}
