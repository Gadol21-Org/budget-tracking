//! Test databases. Every test gets its own empty database on a MySQL-compatible server:
//! the one `MYSQL_URL` points at, or else a throwaway `mariadbd`/`mysqld` started from the
//! PATH in a temp directory (stopped and deleted when the test run ends).

use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::mysql::MySqlStorage;

static SERVER: OnceLock<String> = OnceLock::new();
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// An empty database (schema created) of its own.
pub async fn fresh() -> MySqlStorage {
    let server = SERVER.get_or_init(server_url);
    let name = format!(
        "budget_test_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let url = format!("{}/{name}", server.trim_end_matches('/'));
    MySqlStorage::connect_url(&url).await.unwrap()
}

fn server_url() -> String {
    if let Ok(url) = std::env::var("MYSQL_URL") {
        // Only the server part matters; each test creates its own database.
        let end = url
            .rfind('/')
            .filter(|&i| i > url.find("://").unwrap_or(0) + 2);
        return end.map_or(url.clone(), |i| url[..i].to_string());
    }
    start_throwaway().unwrap_or_else(|e| {
        panic!(
            "no MySQL for the tests: {e}\n\
             Set MYSQL_URL (e.g. `docker compose up -d` and MYSQL_URL=mysql://root:budget@127.0.0.1:3306/budget) \
             or install mariadb-server / mysql-server so `mariadbd` or `mysqld` is on the PATH."
        )
    })
}

fn on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn start_throwaway() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("budget-test-mysql-{}", std::process::id()));
    let data = dir.join("data");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let port = TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map_err(|e| e.to_string())?
        .port();

    let (server, init): (&str, Vec<String>) = if on_path("mariadbd") {
        let install = if on_path("mariadb-install-db") {
            "mariadb-install-db"
        } else {
            "mysql_install_db"
        };
        (
            "mariadbd",
            vec![
                install.into(),
                "--no-defaults".into(),
                format!("--datadir={}", data.display()),
                "--auth-root-authentication-method=normal".into(),
                "--skip-test-db".into(),
            ],
        )
    } else if on_path("mysqld") {
        (
            "mysqld",
            vec![
                "mysqld".into(),
                "--no-defaults".into(),
                "--initialize-insecure".into(),
                format!("--datadir={}", data.display()),
            ],
        )
    } else {
        return Err("neither mariadbd nor mysqld is on the PATH".into());
    };
    let out = Command::new(&init[0])
        .args(&init[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", init[0]))?;
    if !out.status.success() {
        return Err(format!(
            "{} failed: {}",
            init[0],
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // mysqld refuses to run as root unless told which user to use.
    let me = Command::new("id")
        .arg("-un")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let log = dir.join("server.log");
    let mut child = Command::new(server)
        .args(["--no-defaults", "--skip-grant-tables"])
        .arg(format!("--user={me}"))
        .arg(format!("--datadir={}", data.display()))
        .arg(format!("--socket={}", dir.join("sock").display()))
        .arg(format!("--pid-file={}", dir.join("pid").display()))
        .arg("--bind-address=127.0.0.1")
        .args(if server == "mysqld" {
            Some("--mysqlx=OFF")
        } else {
            None
        })
        .arg(format!("--port={port}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("{server}: {e}"))?;
    watch_and_clean_up(child.id(), &dir);

    let deadline = Instant::now() + Duration::from_secs(60);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        let exited = matches!(child.try_wait(), Ok(Some(_)));
        if exited || Instant::now() > deadline {
            return Err(format!(
                "{server} did not start listening on port {port}:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(format!("mysql://root@127.0.0.1:{port}"))
}

/// Stops the server and deletes its directory once this test process is gone (a static
/// is never dropped, so a detached shell watches the process instead).
fn watch_and_clean_up(server_pid: u32, dir: &Path) {
    let script = format!(
        "while kill -0 {me} 2>/dev/null; do sleep 1; done; \
         kill {server_pid} 2>/dev/null; sleep 2; rm -rf '{dir}'",
        me = std::process::id(),
        dir = dir.display()
    );
    let _ = Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}
