//! Quick-launch shortcuts for popular database GUI clients.
//!
//! Detects common database client apps on the host (macOS app bundle paths,
//! Linux .desktop / PATH lookups, Windows program paths) and exposes them as a
//! list of [`DbClient`] entries. Clicking launches the app via the platform's
//! open handler. Verve does not manage connections itself — this is purely a
//! convenience launcher so developers can pop open their DB GUI from the same
//! workbench they're testing APIs in.

use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DbClient {
    pub name: &'static str,
    pub vendor: &'static str, // postgres / mysql / redis / sqlite / generic
    /// Absolute path (or command name) to launch.
    pub launch_path: PathBuf,
}

/// Return the list of installed DB clients detected on this host.
pub fn detect() -> Vec<DbClient> {
    #[cfg(target_os = "macos")]
    {
        detect_macos()
    }
    #[cfg(target_os = "linux")]
    {
        detect_linux()
    }
    #[cfg(target_os = "windows")]
    {
        detect_windows()
    }
}

#[cfg(target_os = "macos")]
fn detect_macos() -> Vec<DbClient> {
    let apps: &[(&str, &str, &str, &str)] = &[
        ("TablePlus", "generic", "/Applications/TablePlus.app", "postgres"),
        ("Postico 2", "postgres", "/Applications/Postico 2.app", "postgres"),
        ("Postico", "postgres", "/Applications/Postico.app", "postgres"),
        ("Sequel Pro", "mysql", "/Applications/Sequel Pro.app", "mysql"),
        ("Sequel Ace", "mysql", "/Applications/Sequel Ace.app", "mysql"),
        ("DBeaver", "generic", "/Applications/DBeaver.app", "generic"),
        ("DBeaverLite", "generic", "/Applications/DBeaverLite.app", "generic"),
        ("MySQLWorkbench", "mysql", "/Applications/MySQLWorkbench.app", "mysql"),
        ("pgAdmin 4", "postgres", "/Applications/pgAdmin 4.app", "postgres"),
        ("RedisInsight", "redis", "/Applications/RedisInsight.app", "redis"),
        ("RedisInsight-v2", "redis", "/Applications/RedisInsight-v2.app", "redis"),
        ("DataGrip", "generic", "/Applications/JetBrains Toolbox/DataGrip.app", "generic"),
        ("DB Browser for SQLite", "sqlite", "/Applications/DB Browser for SQLite.app", "sqlite"),
    ];
    let mut out = Vec::new();
    for (name, vendor, path, _tag) in apps {
        let p = PathBuf::from(path);
        if p.exists() {
            out.push(DbClient {
                name,
                vendor,
                launch_path: p,
            });
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn detect_linux() -> Vec<DbClient> {
    let bins: &[(&str, &str, &str)] = &[
        ("DBeaver", "generic", "dbeaver"),
        ("MySQL Workbench", "mysql", "mysql-workbench"),
        ("pgAdmin 4", "postgres", "pgadmin4"),
        ("DB Browser for SQLite", "sqlite", "sqlitebrowser"),
        ("Redis Insight", "redis", "redisinsight"),
        ("DataGrip", "generic", "datagrip"),
    ];
    bins.iter()
        .filter_map(|(name, vendor, bin)| which(bin).map(|p| DbClient {
            name,
            vendor,
            launch_path: p,
        }))
        .collect()
}

#[cfg(target_os = "windows")]
fn detect_windows() -> Vec<DbClient> {
    let paths: &[(&str, &str, &str)] = &[
        ("DBeaver", "generic", r"C:\Program Files\DBeaver\dbeaver.exe"),
        ("MySQL Workbench", "mysql", r"C:\Program Files\MySQL\MySQL Workbench 8.0 CE\MySQLWorkbench.exe"),
        ("pgAdmin 4", "postgres", r"C:\Program Files\pgAdmin 4\v8\runtime\pgAdmin4.exe"),
        ("DB Browser for SQLite", "sqlite", r"C:\Program Files\DB Browser for SQLite\DB Browser for SQLite.exe"),
        ("DataGrip", "generic", r"C:\Program Files\JetBrains\DataGrip\bin\datagrip64.exe"),
        ("TablePlus", "generic", r"C:\Program Files\TablePlus\TablePlus.exe"),
    ];
    paths
        .iter()
        .filter_map(|(name, vendor, p)| {
            let path = PathBuf::from(p);
            if path.exists() {
                Some(DbClient { name, vendor, launch_path: path })
            } else {
                None
            }
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path_var| {
        std::env::split_paths(&path_var).find_map(|dir| {
            let p = dir.join(bin);
            if p.exists() { Some(p) } else { None }
        })
    })
}

/// Launch a client; non-blocking. On macOS we use `open -a <app>.app` so the
/// bundle is started by LaunchServices. Elsewhere we just spawn the binary.
pub fn launch(client: &DbClient) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .arg("-a")
            .arg(&client.launch_path)
            .spawn()?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new(&client.launch_path).spawn()?;
    }
    Ok(())
}
