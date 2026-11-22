//! `doctor [rt…]`: can this machine run the runtimes? Kernel switches for
//! user namespaces, the runtime's own bubblewrap creating one, and its
//! `steam-runtime-check-requirements` and `steam-runtime-system-info`.

use libsruman::{
    catalog::Arch,
    diag::{self, Finding, Kernel, Level, TOOL_TIMEOUT},
    launch,
    model::{self, SlrMode},
    version::RuntimeRef,
};

use crate::app::{App, AppError};

/// Checks the system, then each runtime: the named ones, or every
/// installed runtime's current build. Exit 1 on an error finding.
pub fn doctor(app: &mut App, targets: &[RuntimeRef]) -> Result<u8, AppError> {
    let store = app.store();
    let targets: Vec<RuntimeRef> = if targets.is_empty() {
        let mut names: Vec<String> = store.installed()?.into_iter().map(|b| b.name).collect();
        names.dedup();
        names
            .into_iter()
            .map(|name| RuntimeRef { name, build: None })
            .collect()
    } else {
        targets.to_vec()
    };
    let arch = Arch::current()
        .ok_or_else(|| AppError::Failed("this architecture has no Steam Linux Runtime".into()))?;

    let mut runtimes = Vec::new();
    let mut worst = Level::Ok;
    for target in &targets {
        let mut profile = model::new_profile(&target.name, target);
        if let Some(slr) = profile.slr.as_mut() {
            slr.mode = SlrMode::Container;
        }
        let Some(slr) = profile.slr.as_ref() else {
            continue;
        };
        let mut problems = Vec::new();
        match launch::resolve_runtimes(slr, &app.launch_ctx(&store, false, &[]), &mut problems) {
            Some(resolved) => runtimes.push((target.clone(), resolved)),
            None => {
                worst = worst.max(Level::Error);
                for problem in problems {
                    app.out.error(format!("{target}: {problem}"))?;
                }
            }
        }
    }

    // The kernel's switches, explained by the first container's bwrap.
    let tools: Vec<_> = runtimes
        .iter()
        .map(|(target, resolved)| (target, diag::tools_for(resolved, arch)))
        .collect();
    let bwrap = tools
        .iter()
        .find_map(|(_, tools)| tools.as_ref().and_then(diag::Tools::bwrap));
    app.out.heading("System")?;
    worst = worst.max(print(app, &Kernel::read().findings(bwrap.as_deref()))?);

    for ((target, resolved), (_, tools)) in runtimes.iter().zip(&tools) {
        app.out.heading(format!(
            "{} {}",
            resolved.runtime.name, resolved.runtime.build
        ))?;
        let Some(tools) = tools else {
            app.out.warning(format!(
                "{target}: no diagnostic tools for {arch} in this build; not checked"
            ))?;
            continue;
        };
        let mut findings = vec![
            diag::bwrap_probe(tools, TOOL_TIMEOUT),
            diag::check_requirements(tools, TOOL_TIMEOUT),
        ];
        match diag::system_info(tools, TOOL_TIMEOUT) {
            Ok(info) => findings.extend(info.findings()),
            Err(err) => app.out.warning(err.to_string())?,
        }
        worst = worst.max(print(app, &findings)?);
    }
    if targets.is_empty() {
        app.out
            .info("no runtimes installed; `sruman install sniper` to check one")?;
    }
    Ok(u8::from(worst == Level::Error))
}

/// Prints findings; returns the worst level.
fn print(app: &mut App, findings: &[Finding]) -> Result<Level, AppError> {
    for finding in findings {
        let label = match finding.level {
            Level::Ok => app.out.green("ok"),
            Level::Warn => app.out.yellow("warning"),
            Level::Error => app.out.red("error"),
        };
        app.out
            .line(format!(" [{label}] {}: {}", finding.check, finding.message))?;
        if let Some(hint) = &finding.hint
            && finding.level != Level::Ok
        {
            app.out.line(format!("        {hint}"))?;
        }
    }
    Ok(diag::worst(findings))
}
