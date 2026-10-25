use shellmcp_agent::{fsops, policy};

#[test]
fn writes_follow_symlinks_and_keep_permissions() {
    let d = tempfile::tempdir().unwrap();
    let target = d.path().join("real.txt");
    std::fs::write(&target, "a\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = d.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        fsops::write_file(&link, b"b\n").unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "b\n");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }
    let fresh = d.path().join("new.txt");
    let v = fsops::write_file(&fresh, b"x").unwrap();
    assert_eq!(v["created"], true);
    assert!(fsops::write_file(&d.path().join("missing/dir/f"), b"x").is_err());
}

#[test]
fn edit_reports_diff_ambiguity_and_nearest_line() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("f.rs");
    std::fs::write(
        &f,
        "fn main() {\n    println!(\"hi\");\n}\nlet x = 1;\nlet x = 1;\n",
    )
    .unwrap();
    let v = fsops::edit(&f, "\"hi\"", "\"hello\"", false).unwrap();
    assert!(
        v["diff"]
            .as_str()
            .unwrap()
            .contains("+    println!(\"hello\");")
    );
    let err = fsops::edit(&f, "let x = 1;", "let x = 2;", false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("2 times"), "{err}");
    assert_eq!(
        fsops::edit(&f, "let x = 1;", "let x = 2;", true).unwrap()["replacements"],
        2
    );
    let err = fsops::edit(&f, "println!(\"helo\");", "", false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Closest line 2"), "{err}");
}

#[test]
fn glob_and_grep_walk_tree_skipping_vendor_dirs() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("src/a")).unwrap();
    std::fs::create_dir_all(d.path().join("node_modules/x")).unwrap();
    std::fs::write(d.path().join("src/a/lib.rs"), "pub fn needle() {}\n").unwrap();
    std::fs::write(d.path().join("src/main.rs"), "fn main() { needle() }\n").unwrap();
    std::fs::write(d.path().join("node_modules/x/i.rs"), "needle\n").unwrap();
    std::fs::write(d.path().join("bin.dat"), b"needle\0\0").unwrap();
    let g = fsops::glob(d.path(), "*.rs", 100).unwrap();
    assert_eq!(g["matches"].as_array().unwrap().len(), 2);
    let g = fsops::glob(d.path(), "src/**/*.{rs,toml}", 100).unwrap();
    assert_eq!(g["matches"].as_array().unwrap().len(), 2);
    let g = fsops::glob(d.path(), "src/*.rs", 100).unwrap();
    assert_eq!(g["matches"][0]["path"], "src/main.rs");
    let r = fsops::grep(d.path(), "NEEDLE", Some("*.rs"), true, 100).unwrap();
    assert_eq!(r["matches"].as_array().unwrap().len(), 2);
    let r = fsops::grep(d.path(), "needle", None, false, 1).unwrap();
    assert_eq!(r["truncated"], true);
}

#[test]
fn policy_modes_and_data_dir_guard() {
    let p = policy::Policy {
        mode: policy::Mode::ReadOnly,
        deny_tools: vec!["fs_grep".into()],
    };
    assert!(policy::check(&p, "fs_read").is_ok());
    assert!(policy::check(&p, "job_output").is_ok());
    assert!(policy::check(&p, "fs_write").is_err());
    assert!(policy::check(&p, "shell_exec").is_err());
    assert!(policy::check(&p, "fs_grep").is_err());
    let off = policy::Policy {
        mode: policy::Mode::Disabled,
        deny_tools: vec![],
    };
    assert!(policy::allowed_tools(&off).is_empty());
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    assert!(policy::guard_path(&data, &data.join("config.json")).is_err());
    assert!(policy::guard_path(&data, &data.join("new/../x")).is_err());
    assert!(policy::guard_path(&data, &d.path().join("other.txt")).is_ok());
    #[cfg(unix)]
    {
        let link = d.path().join("sneaky");
        std::os::unix::fs::symlink(&data, &link).unwrap();
        assert!(policy::guard_path(&data, &link.join("policy.json")).is_err());
        assert!(policy::guard_path(&data, &link.join("missing/../policy.json")).is_err());
    }
}

#[test]
fn login_env_parser_ignores_profile_noise() {
    let raw = b"motd banner\n\0__SHELLMCP_ENV__\0PATH=/a:/b\0HOME=/h\0PWD=/x\0MULTI=a\nb\0";
    let env = shellmcp_agent::environment::parse(raw).unwrap();
    assert_eq!(env["PATH"], "/a:/b");
    assert_eq!(env["MULTI"], "a\nb");
    assert!(!env.contains_key("PWD"));
    assert!(shellmcp_agent::environment::parse(b"PATH=/a").is_err());
}
