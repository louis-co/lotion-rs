//! Debug-build diagnostics: with `LOTION_DIAG=1`, every tab periodically
//! reports its page state into the log. The report travels through
//! `document.title` because Notion pages intentionally have no IPC access.
//! Compiled out of release builds.

use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

pub const TITLE_PREFIX: &str = "__lotion_diag__:";

const PROBE: &str = r#"(function(){
  try {
    if (!window.__lotionKeys) {
      window.__lotionKeys = [];
      addEventListener('keydown', function(e){
        window.__lotionKeys.push((e.metaKey?'Cmd+':'')+(e.ctrlKey?'Ctrl+':'')+(e.shiftKey?'Shift+':'')+e.key);
        if (window.__lotionKeys.length > 20) window.__lotionKeys.shift();
      }, true);
    }
    if (window.__lotionResizes === undefined) {
      window.__lotionResizes = 0;
      addEventListener('resize', function(){ window.__lotionResizes++; });
    }
    var body = document.body ? document.body.innerText.slice(0, 4000) : '';
    var d = {
      href: location.href, ready: document.readyState,
      viewport: [innerWidth, innerHeight, devicePixelRatio],
      ua: navigator.userAgent,
      notionApp: !!document.querySelector('#notion-app'),
      loggedOutLanding: !!document.querySelector('a[href*="/login"]'),
      opfs: !!(navigator.storage && navigator.storage.getDirectory),
      sharedWorker: typeof SharedWorker !== 'undefined',
      crossOriginIsolated: !!self.crossOriginIsolated,
      blocks: document.querySelectorAll('[data-block-id]').length,
      dbRows: document.querySelectorAll('.notion-collection-item, .notion-table-view-row').length,
      lotionTheme: !!document.getElementById('lotion-theme-style'),
      unsupported: /(browser|version) (is )?not supported|unsupported browser|update your browser/i.test(body),
      sw: 'serviceWorker' in navigator,
      swControlled: !!(navigator.serviceWorker && navigator.serviceWorker.controller),
      hasFocus: document.hasFocus(),
      keys: window.__lotionKeys,
      resizes: window.__lotionResizes,
      hidden: document.hidden,
      heapMB: performance.memory ? Math.round(performance.memory.usedJSHeapSize / 1048576) : null
    };
    var t = document.title;
    document.title = '__lotion_diag__:' + JSON.stringify(d);
    // WebKit coalesces synchronous title changes; restore on a later tick.
    setTimeout(function(){ document.title = t; }, 250);
  } catch (e) {}
})();"#;

pub fn start<R: Runtime>(app: &AppHandle<R>) {
    let Ok(interval) = std::env::var("LOTION_DIAG") else {
        return;
    };
    let secs = interval.parse::<u64>().unwrap_or(10).max(2);
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(secs));
        for (label, webview) in app.webviews() {
            if label != crate::window_controller::MAIN_WINDOW {
                let _ = webview.eval(PROBE);
            }
        }
    });
}

/// `LOTION_SELFTEST=1`: drive the tab actions the menu/tab strip use and log
/// the resulting tab list after each step.
pub fn start_selftest<R: Runtime>(app: &AppHandle<R>) {
    if std::env::var("LOTION_SELFTEST").is_err() {
        return;
    }
    use crate::window_controller as wc;
    type Step<R> = (&'static str, fn(&AppHandle<R>));
    let steps: Vec<Step<R>> = vec![
        ("new_tab", |a| wc::new_tab(a, None)),
        ("new_tab_2", |a| wc::new_tab(a, None)),
        ("select_prev", |a| wc::select_relative_tab(a, -1)),
        ("select_last(cmd9)", |a| wc::select_tab_number(a, 9)),
        ("select_first(cmd1)", |a| wc::select_tab_number(a, 1)),
        ("move_first_to_end", |a| {
            if let Some(o) = a.try_state::<std::sync::Arc<dyn crate::traits::TabOrchestrator<R>>>()
            {
                if let Some(id) = o.get_tab_ids().first().cloned() {
                    wc::move_tab(a, &id, 99);
                }
            }
        }),
        ("close_active", |a| wc::close_active_tab(a)),
        ("reopen_closed", |a| wc::reopen_closed_tab(a)),
        ("zoom_in", |a| wc::zoom(a, 1)),
        ("zoom_in", |a| wc::zoom(a, 1)),
        ("zoom_reset", |a| wc::zoom(a, 0)),
        ("force_reload", |a| wc::force_reload_active(a)),
        ("reload", |a| wc::reload_active(a)),
        ("suspend_idle(0s)", |a| {
            if let Some(o) = a.try_state::<std::sync::Arc<dyn crate::traits::TabOrchestrator<R>>>()
            {
                o.suspend_idle_tabs(Duration::ZERO);
            }
        }),
        ("select_next(resume)", |a| wc::select_relative_tab(a, 1)),
        ("live_resize_x40", |a| {
            // Simulate a live window resize: 40 size changes.
            if let Some(w) = a.get_window(crate::window_controller::MAIN_WINDOW) {
                for i in 0..40 {
                    let _ = w.set_size(tauri::LogicalSize::new(1100.0 + (i * 5) as f64, 760.0));
                }
            }
        }),
    ];
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        for (name, step) in steps {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                step(&handle);
                if let Some(o) =
                    handle.try_state::<std::sync::Arc<dyn crate::traits::TabOrchestrator<R>>>()
                {
                    let tabs: Vec<String> = o
                        .tab_states()
                        .iter()
                        .map(|t| format!("{}{}", &t.id[..4], if t.is_active { "*" } else { "" }))
                        .collect();
                    log::info!("[selftest] {:<20} -> {:?}", name, tabs);
                }
            });
            std::thread::sleep(Duration::from_millis(1500));
        }
        log::info!("[selftest] done");
    });
}

/// `LOTION_SCROLLBENCH=<delay secs>`: run docs/benchmarks/scroll-bench.js in
/// the active tab once and log the result.
pub fn start_scroll_bench<R: Runtime>(app: &AppHandle<R>) {
    let Ok(delay) = std::env::var("LOTION_SCROLLBENCH") else {
        return;
    };
    let delay = delay.parse::<u64>().unwrap_or(20);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(delay));
        let Some(orchestrator) = app
            .try_state::<std::sync::Arc<dyn crate::traits::TabOrchestrator<R>>>()
            .map(|o| o.inner().clone())
        else {
            return;
        };
        if let Some(webview) = orchestrator.active_webview() {
            let script = format!(
                "({}\n)().then(function(r){{ var t=document.title; document.title={:?}+JSON.stringify(r); setTimeout(function(){{document.title=t}},250); }});",
                include_str!("../../docs/benchmarks/scroll-bench.js").trim().trim_end_matches(';').trim_end_matches("()"),
                TITLE_PREFIX
            );
            log::info!("[diag] running scroll benchmark in active tab");
            let _ = webview.eval(script);
        }
    });
}
