use crate::config::Config;
use crate::errors::Result;
use crate::logging;
use crate::{diff, discover, manifest::Manifest, scan, sync};
use notify::{RecursiveMode, Watcher};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// How long to wait before looking again at files that were still being
/// written during the last cycle (just past scan's quiet window).
const RECHECK_SECS: u64 = 3;

/// Watch the sessions root (recursively, so brand-new account folders are
/// picked up too) and run scan+sync after each debounced burst of events.
pub fn watch(cfg: &Config, dry_run: bool, no_backup: bool) -> Result<()> {
    let (tx, rx) = mpsc::channel::<()>();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            // Ignore our own temp files to avoid feedback loops.
            let ours = event.paths.iter().all(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e == "meld-tmp" || e == "meld-restore-tmp")
                    .unwrap_or(false)
            });
            if !ours {
                let _ = tx.send(());
            }
        }
    })?;
    watcher.watch(&cfg.sessions_root, RecursiveMode::Recursive)?;

    logging::info("Watching your Claude Code sessions. New conversations will be shared across all accounts automatically. Press Ctrl-C to stop.");
    logging::verbose(&format!(
        "watching {} (debounce {}ms)",
        cfg.sessions_root.display(),
        cfg.debounce_ms
    ));

    // Initial pass so a fresh `meld watch` converges immediately.
    let mut recheck = run_cycle(cfg, dry_run, no_backup)?;

    loop {
        // Block until something changes — or, if the last cycle had to skip
        // files that were mid-write, until it's time to look at them again.
        if recheck {
            match rx.recv_timeout(Duration::from_secs(RECHECK_SECS)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else if rx.recv().is_err() {
            break;
        }
        // ...then drain further events until things go quiet (debounce).
        while rx
            .recv_timeout(Duration::from_millis(cfg.debounce_ms))
            .is_ok()
        {}
        recheck = match run_cycle(cfg, dry_run, no_backup) {
            Ok(r) => r,
            Err(e) => {
                logging::warn(&format!("sync cycle failed: {e:#}"));
                false
            }
        };
    }
    Ok(())
}

/// One scan+sync pass. Returns true when some files were skipped because
/// they were still being written.
fn run_cycle(cfg: &Config, dry_run: bool, no_backup: bool) -> Result<bool> {
    let roots = discover::discover_account_roots(cfg)?;
    let previous = Manifest::load_or_default(&cfg.manifest_path());
    let mut m = scan::scan(cfg, &roots, &previous)?;
    let report = diff::compute(&m, cfg.policy());
    if !report.is_empty() {
        sync::apply(cfg, &report, dry_run, no_backup)?;
        // Rescan after writing so the manifest reflects reality.
        m = scan::scan(cfg, &roots, &m)?;
    }
    m.save(&cfg.manifest_path())?;
    Ok(m.skipped_recent > 0)
}
