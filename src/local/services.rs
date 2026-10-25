use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub path: PathBuf,
    pub content: String,
}
fn encoded(content: &str) -> Vec<u8> {
    #[cfg(windows)]
    {
        std::iter::once(0xfeffu16)
            .chain(content.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect()
    }
    #[cfg(not(windows))]
    {
        content.as_bytes().to_vec()
    }
}
pub fn checked(command: &mut Command) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "service command failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
fn clean(s: &str) -> Result<()> {
    ensure!(!s.contains(['\n', '\r', '\0']), "invalid service argument");
    Ok(())
}
#[cfg(target_os = "linux")]
fn quote(s: &str) -> Result<String> {
    clean(s)?;
    Ok(format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}
#[cfg(any(target_os = "macos", windows))]
fn xml(s: &str) -> Result<String> {
    clean(s)?;
    Ok(s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}
pub fn definitions(dir: &Path, release: &Path, instance: &str) -> Result<Vec<Definition>> {
    let dir = dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 install path"))?;
    let binary = super::download::executable(
        release,
        if cfg!(windows) {
            "shellmcp-host"
        } else {
            "shellmcp-agent"
        },
    );
    let binary = binary
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF8 binary path"))?;
    let mut result = vec![];
    for (kind, action) in [("agent", "run"), ("tunnel", "tunnel-run")] {
        let name = format!("shellmcp-local-{}-{kind}", &instance[..12]);
        let args = [
            binary,
            "local",
            action,
            "--data-dir",
            dir,
            "--release-dir",
            release.to_str().unwrap(),
        ];
        #[cfg(target_os = "linux")]
        {
            let path = crate::runtime::home_dir()
                .join(".config/systemd/user")
                .join(format!("{name}.service"));
            let args = args
                .iter()
                .map(|s| quote(s))
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            let kill_mode = if kind == "agent" {
                "process"
            } else {
                "control-group"
            };
            let content = format!(
                "[Unit]\nDescription=ShellMCP local {kind}\nAfter=network-online.target\n[Service]\nType=simple\nExecStart={args}\nRestart=on-failure\nRestartSec=3\nKillMode={kill_mode}\nTimeoutStopSec=10\nUMask=0077\n[Install]\nWantedBy=default.target\n"
            );
            result.push(Definition {
                name,
                path,
                content,
            });
        }
        #[cfg(target_os = "macos")]
        {
            let name = format!("com.{name}");
            let path = crate::runtime::home_dir()
                .join("Library/LaunchAgents")
                .join(format!("{name}.plist"));
            let args = args
                .iter()
                .map(|s| Ok(format!("<string>{}</string>", xml(s)?)))
                .collect::<Result<Vec<_>>>()?
                .join("");
            let abandon = if kind == "agent" {
                "<true/>"
            } else {
                "<false/>"
            };
            let log = xml(&format!("{dir}/{kind}.log"))?;
            let content = format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Label</key><string>{name}</string><key>ProgramArguments</key><array>{args}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>AbandonProcessGroup</key>{abandon}<key>StandardOutPath</key><string>{log}</string><key>StandardErrorPath</key><string>{log}</string></dict></plist>"
            );
            result.push(Definition {
                name,
                path,
                content,
            });
        }
        #[cfg(windows)]
        {
            // Do not use cmd.exe, PowerShell -WindowStyle or a console executable as task action.
            let path = Path::new(dir).join(format!("{name}.xml"));
            use std::os::windows::process::CommandExt;
            let identity = Command::new("whoami")
                .args(["/user", "/fo", "csv", "/nh"])
                .creation_flags(0x08000000)
                .output()?;
            ensure!(identity.status.success(), "cannot resolve current user SID");
            let output = String::from_utf8(identity.stdout)?;
            let account = output
                .trim()
                .rsplit(',')
                .next()
                .unwrap_or_default()
                .trim_matches('"');
            ensure!(
                account.starts_with("S-1-")
                    && account[2..].chars().all(|c| c.is_ascii_digit() || c == '-'),
                "invalid current user SID"
            );
            let account = xml(account)?;
            let arguments = args[1..]
                .iter()
                .map(|s| {
                    clean(s)?;
                    ensure!(
                        !s.contains('"') && !s.ends_with('\\'),
                        "invalid Windows task argument"
                    );
                    Ok(format!("\"{s}\""))
                })
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            let content = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-16\"?><Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\"><RegistrationInfo><Description>ShellMCP local {}</Description></RegistrationInfo><Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{account}</UserId></LogonTrigger></Triggers><Principals><Principal id=\"User\"><UserId>{account}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><RestartOnFailure><Interval>PT1M</Interval><Count>999</Count></RestartOnFailure><Enabled>true</Enabled><Hidden>true</Hidden></Settings><Actions Context=\"User\"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions></Task>",
                xml(dir)?,
                xml(binary)?,
                xml(&arguments)?
            );
            result.push(Definition {
                name,
                path,
                content,
            });
        }
    }
    Ok(result)
}
pub fn preflight(defs: &[Definition], old: &[Definition]) -> Result<()> {
    #[cfg(target_os = "linux")]
    checked(Command::new("systemctl").args(["--user", "show-environment"]))?;
    #[cfg(target_os = "macos")]
    checked(
        Command::new("launchctl").args(["print", &format!("gui/{}", unsafe { libc::geteuid() })]),
    )?;
    for d in defs {
        if d.path.exists() {
            let previous = old
                .iter()
                .find(|p| p.name == d.name)
                .ok_or_else(|| anyhow::anyhow!("refusing unowned service {}", d.name))?;
            ensure!(
                std::fs::read(&d.path)? == encoded(&previous.content),
                "service definition changed outside this installation: {}",
                d.name
            );
        }
        #[cfg(windows)]
        {
            let exists = Command::new("schtasks")
                .args(["/Query", "/TN", &d.name])
                .output()?
                .status
                .success();
            ensure!(
                !exists || old.iter().any(|p| p.name == d.name),
                "unowned scheduled task {}",
                d.name
            );
        }
    }
    Ok(())
}
pub fn install(d: &Definition) -> Result<()> {
    std::fs::create_dir_all(d.path.parent().unwrap())?;
    crate::runtime::private_write(&d.path, &encoded(&d.content))?;
    #[cfg(target_os = "linux")]
    {
        checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        checked(Command::new("systemctl").args(["--user", "enable", &d.name]))?;
        checked(Command::new("systemctl").args(["--user", "restart", &d.name]))?;
    }
    #[cfg(target_os = "macos")]
    {
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{domain}/{}", d.name)])
            .output();
        checked(
            Command::new("launchctl")
                .arg("bootstrap")
                .arg(domain)
                .arg(&d.path),
        )?;
    }
    #[cfg(windows)]
    {
        checked(
            Command::new("schtasks")
                .args(["/Create", "/F", "/TN", &d.name, "/XML"])
                .arg(&d.path),
        )?;
        checked(Command::new("schtasks").args(["/Run", "/TN", &d.name]))?;
    }
    Ok(())
}
pub fn stop(d: &Definition) -> Result<()> {
    #[cfg(target_os = "linux")]
    checked(Command::new("systemctl").args(["--user", "stop", &d.name]))?;
    #[cfg(target_os = "macos")]
    checked(Command::new("launchctl").args([
        "bootout",
        &format!("gui/{}/{}", unsafe { libc::geteuid() }, d.name),
    ]))?;
    #[cfg(windows)]
    {
        checked(Command::new("schtasks").args(["/End", "/TN", &d.name]))?;
        use std::os::windows::process::CommandExt;
        let script = format!(
            "$s=New-Object -ComObject Schedule.Service; $s.Connect(); if($s.GetFolder('\\').GetTask('{}').State -eq 4){{exit 4}}",
            d.name
        );
        for i in 0..20 {
            let out = Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .creation_flags(0x08000000)
                .output()?;
            if out.status.success() {
                break;
            }
            ensure!(i < 19, "scheduled task did not stop: {}", d.name);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    Ok(())
}
pub fn remove(d: &Definition) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        checked(Command::new("systemctl").args(["--user", "disable", "--now", &d.name]))?;
        if d.path.exists() {
            std::fs::remove_file(&d.path)?;
        }
        checked(Command::new("systemctl").args(["--user", "daemon-reload"]))?;
        let output = Command::new("systemctl")
            .args(["--user", "show", &d.name, "-p", "MainPID", "--value"])
            .output()?;
        ensure!(
            String::from_utf8_lossy(&output.stdout).trim() == "0",
            "service still has a process"
        );
    }
    #[cfg(target_os = "macos")]
    {
        stop(d)?;
        std::fs::remove_file(&d.path)?;
    }
    #[cfg(windows)]
    {
        stop(d)?;
        checked(Command::new("schtasks").args(["/Delete", "/F", "/TN", &d.name]))?;
        std::fs::remove_file(&d.path)?;
    }
    Ok(())
}
