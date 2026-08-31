use std::path::PathBuf;

fn argument(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("snapshot") {
        eprintln!(
            "usage: dsh-desktop-diagnostics snapshot [--app-data PATH] [--dsh-home PATH] [--profile NAME] [--core PATH --profile-path PATH]"
        );
        std::process::exit(2);
    }
    let app_data = argument(&args, "--app-data").map_or_else(
        || {
            PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default())
                .join("io.github.hairyf.deepseek-harness-desktop")
        },
        PathBuf::from,
    );
    let dsh_home = argument(&args, "--dsh-home").map_or_else(
        || PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(".dsh"),
        PathBuf::from,
    );
    let profile = argument(&args, "--profile").unwrap_or_else(|| "product-zlzhg".to_string());
    let snapshot = match (argument(&args, "--core"), argument(&args, "--profile-path")) {
        (Some(core), Some(profile)) => {
            main::diagnostics::snapshot_for_paths(&PathBuf::from(core), &PathBuf::from(profile))
        }
        (None, None) => main::diagnostics::snapshot_from_roots(&app_data, &dsh_home, &profile),
        _ => Err("DIAGNOSTICS_ARGUMENTS: --core and --profile-path must be used together".into()),
    };
    match snapshot {
        Ok(snapshot) => println!("{}", serde_json::to_string_pretty(&snapshot).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
