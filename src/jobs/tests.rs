use super::*;
use std::time::Duration;

struct Fixture {
    dir: tempfile::TempDir,
    id: String,
    pid: u32,
}

impl Fixture {
    async fn new() -> (Self, tokio::process::Child) {
        let dir = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(job_dir(dir.path(), &id).unwrap()).unwrap();
        Store::open(dir.path()).unwrap().new_job(&id, &id).unwrap();
        // A real child stays alive until released, independent of the request's
        // lifetime. Bound its lifetime even if a test fails before releasing it.
        let child = tokio::process::Command::new("sh")
            .args([
                "-c",
                "printf ready > \"$1/ready\"; i=0; while [ ! -e \"$1/release\" ]; do i=$((i+1)); [ \"$i\" -lt 500 ] || exit 99; sleep 0.02; done; printf survived > \"$1/finished\"",
                "job-fixture",
            ])
            .arg(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let fixture = Self {
            dir,
            id,
            pid: child.id().unwrap(),
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while !fixture.dir.path().join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        (fixture, child)
    }

    fn store(&self) -> Store {
        Store::open(self.dir.path()).unwrap()
    }

    fn release(&self) {
        std::fs::write(self.dir.path().join("release"), b"").unwrap();
    }

    async fn assert_reaped(&self) {
        let path = std::path::PathBuf::from(format!("/proc/{}", self.pid));
        let reaped = tokio::time::timeout(Duration::from_secs(3), async {
            while path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        // Clean up a zombie on regression failures, without masking the failure.
        if !reaped {
            unsafe { libc::waitpid(self.pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
        }
        assert!(reaped, "child {} was not reaped", self.pid);
        assert_eq!(
            std::fs::read_to_string(self.dir.path().join("finished")).unwrap(),
            "survived",
            "returning or cancelling a request must not kill the background job"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::write(self.dir.path().join("release"), b"");
    }
}

#[tokio::test]
async fn completed_job_reaps_supervisor_after_return() {
    let (f, child) = Fixture::new().await;
    f.store().job_done(&f.id, 23).unwrap();
    std::fs::write(
        job_dir(f.dir.path(), &f.id).unwrap().join("output.log"),
        b"finished command",
    )
    .unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        wait_for_job(f.dir.path(), &f.id, f.store(), 10, child),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["job"]["exit_code"], 23);
    assert_eq!(result["output"]["output"], "finished command");
    f.release();
    f.assert_reaped().await;
}

#[tokio::test]
async fn wait_timeout_preserves_job_and_reaps_it_later() {
    let (f, child) = Fixture::new().await;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        wait_for_job(f.dir.path(), &f.id, f.store(), 0, child),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result["background"], true);
    assert_eq!(result["job"]["job_id"], f.id);
    f.release();
    f.assert_reaped().await;
}

#[tokio::test]
async fn cancelled_wait_preserves_job_and_reaps_it_later() {
    let (f, child) = Fixture::new().await;
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_job(f.dir.path(), &f.id, f.store(), 10, child),
    )
    .await;
    assert!(
        result.is_err(),
        "request should still be waiting when cancelled"
    );
    f.release();
    f.assert_reaped().await;
}

#[tokio::test]
async fn database_error_still_reaps_supervisor() {
    let (f, child) = Fixture::new().await;
    let store = f.store();
    rusqlite::Connection::open(f.dir.path().join("state.db"))
        .unwrap()
        .execute("DELETE FROM jobs WHERE id=?", [&f.id])
        .unwrap();
    assert!(
        wait_for_job(f.dir.path(), &f.id, store, 10, child)
            .await
            .is_err()
    );
    f.release();
    f.assert_reaped().await;
}

#[tokio::test]
async fn output_error_still_reaps_supervisor() {
    let (f, child) = Fixture::new().await;
    f.store().job_done(&f.id, 0).unwrap();
    std::fs::create_dir(job_dir(f.dir.path(), &f.id).unwrap().join("output.log")).unwrap();
    assert!(
        wait_for_job(f.dir.path(), &f.id, f.store(), 10, child)
            .await
            .is_err()
    );
    f.release();
    f.assert_reaped().await;
}

#[tokio::test]
async fn supervisor_exit_before_completion_is_reported_and_reaped() {
    let (f, child) = Fixture::new().await;
    f.release();
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        wait_for_job(f.dir.path(), &f.id, f.store(), 10, child),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("supervisor exited before completion")
    );
    assert_eq!(f.store().job(&f.id).unwrap()["exit_code"], -1);
    f.assert_reaped().await;
}
