//! Installing and removing the privileged half of local domains. Everything
//! here needs root, so every path through it either has root or says plainly
//! that it does not.

use anyhow::{Result, bail};
use comb::{Error, Layout};

pub async fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("status") | None => status().await,
        Some("install") => install(args.iter().any(|arg| arg == "--take-over")),
        Some("uninstall") => uninstall(),
        Some(other) => bail!("skep domains {other}? try status, install or uninstall"),
    }
}

async fn status() -> Result<()> {
    let layout = Layout::system(comb::SUFFIX);

    match comb::routing(comb::SUFFIX) {
        comb::Routing::Ours => println!("names       .{} comes here", comb::SUFFIX),
        comb::Routing::Missing => println!("names       .{} does not resolve yet", comb::SUFFIX),
        comb::Routing::Elsewhere { says } => {
            println!("names       .{} goes somewhere else ({says})", comb::SUFFIX)
        }
    }

    match comb::health(&layout.control).await {
        Ok(health) => {
            let forwards: Vec<String> = health
                .forwarding
                .iter()
                .map(|forward| format!("{} to {}", forward.from, forward.to))
                .collect();
            println!("ports       held by the helper, {}", forwards.join(" and "));
        }
        Err(error) => println!("ports       {error}"),
    }

    match comb::Authority::open(&comb::Paths::from_env()) {
        Ok(authority) if authority.is_trusted() => println!("certificates trusted"),
        Ok(_) => println!("certificates not trusted yet, run skep trust"),
        Err(error) => println!("certificates {error}"),
    }
    Ok(())
}

fn install(take_over: bool) -> Result<()> {
    if !comb::is_root() {
        bail!(
            "installing local domains needs an administrator, because it writes\n\
             /etc/resolver and a launchd daemon:\n\n  sudo skep domains install"
        );
    }

    let layout = Layout::system(comb::SUFFIX);
    let helper = beside_this_binary("skep-helper")?;

    let touched = match comb::place(&layout, &helper, comb::invoking_user(), take_over) {
        Ok(touched) => touched,
        Err(Error::ResolverInUse { says, likely }) => {
            let mut said = format!("something else already routes .{}\n", comb::SUFFIX);
            said.push_str(&format!("  the file says:  {says}\n"));
            if let Some(likely) = likely {
                said.push_str(&format!("  it looks like:  {likely}\n"));
            }
            said.push_str(
                "\nskep will not take that over on its own. Either remove it, or:\n\
                 \n  sudo skep domains install --take-over\n\
                 \nTaking over keeps a copy of the original and puts it back when you\n\
                 run skep domains uninstall.",
            );
            bail!(said);
        }
        Err(other) => return Err(other.into()),
    };

    for path in &touched {
        println!("wrote {}", path.display());
    }

    // A written file is not working resolution, so say what was actually
    // proved rather than what was attempted.
    match comb::activate(&layout, comb::SUFFIX) {
        Ok(verified) => println!("verified {verified}"),
        Err(error) => {
            bail!("{error}\n\nThe files are in place. Run skep domains uninstall to undo them.")
        }
    }
    Ok(())
}

fn uninstall() -> Result<()> {
    if !comb::is_root() {
        bail!("removing local domains needs an administrator:\n\n  sudo skep domains uninstall");
    }

    let layout = Layout::system(comb::SUFFIX);
    let restoring = layout.backup.is_file();
    if let Err(error) = comb::deactivate(&layout) {
        // A daemon that was never loaded is not a reason to leave files behind.
        println!("note: {error}");
    }

    for path in comb::remove(&layout)? {
        println!("removed {}", path.display());
    }
    if restoring {
        println!("put back the resolver file that was there before skep");
    }
    Ok(())
}

/// The helper ships beside the command that installs it, so a build and an
/// install both find it in the same place.
fn beside_this_binary(name: &str) -> Result<std::path::PathBuf> {
    let here = std::env::current_exe()?;
    let found = here
        .parent()
        .map(|directory| directory.join(name))
        .filter(|path| path.is_file());
    match found {
        Some(path) => Ok(path),
        None => bail!("cannot find {name} next to {}", here.display()),
    }
}
