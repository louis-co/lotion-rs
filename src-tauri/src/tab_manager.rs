use crate::litebox::LiteBox;
use crate::tab_controller::TabController;
use crate::traits::TabOrchestrator;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime};

/// How many closed tabs "Reopen Closed Tab" remembers.
const CLOSED_TAB_HISTORY: usize = 20;

/// Tabs never unloaded regardless of idle time (active + most recently used),
/// matching Notion desktop.
const KEEP_RECENT_TABS: usize = 3;

struct Tabs<R: Runtime> {
    order: Vec<String>,
    map: HashMap<String, Arc<TabController<R>>>,
    active: Option<String>,
    closed: Vec<String>,
}

pub struct TabManager<R: Runtime> {
    tabs: RwLock<Tabs<R>>,
    pub litebox: Arc<LiteBox>,
}

impl<R: Runtime> TabManager<R> {
    pub fn new(litebox: Arc<LiteBox>) -> Self {
        Self {
            tabs: RwLock::new(Tabs {
                order: Vec::new(),
                map: HashMap::new(),
                active: None,
                closed: Vec::new(),
            }),
            litebox,
        }
    }

    fn get(&self, tab_id: &str) -> Option<Arc<TabController<R>>> {
        self.tabs
            .read()
            .expect("TabManager: tabs read lock poisoned")
            .map
            .get(tab_id)
            .cloned()
    }

    fn all(&self) -> Vec<Arc<TabController<R>>> {
        let tabs = self.tabs.read().expect("TabManager: tabs read lock poisoned");
        tabs.order.iter().filter_map(|id| tabs.map.get(id).cloned()).collect()
    }
}

impl<R: Runtime> TabOrchestrator<R> for TabManager<R> {
    fn create_tab(&self, app: &AppHandle<R>, window_id: &str, url: &str) -> tauri::Result<String> {
        let tab_id = uuid::Uuid::new_v4().to_string();

        // Webview creation happens outside the lock: it can re-enter the event
        // loop, which may in turn call back into the manager.
        let tab_controller =
            TabController::new(app, window_id, tab_id.clone(), url, "", false, self.litebox.clone())?;

        let mut tabs = self.tabs.write().expect("TabManager: tabs write lock poisoned");
        let insert_at = tabs
            .active
            .as_ref()
            .and_then(|a| tabs.order.iter().position(|id| id == a))
            .map(|i| i + 1)
            .unwrap_or(tabs.order.len());
        tabs.order.insert(insert_at, tab_id.clone());
        tabs.map.insert(tab_id.clone(), Arc::new(tab_controller));

        log::info!("TabManager: Created tab {}", tab_id);
        Ok(tab_id)
    }

    fn create_lazy_tab(
        &self,
        app: &AppHandle<R>,
        window_id: &str,
        url: &str,
        title: &str,
    ) -> tauri::Result<String> {
        let tab_id = uuid::Uuid::new_v4().to_string();
        let tab_controller =
            TabController::new(app, window_id, tab_id.clone(), url, title, true, self.litebox.clone())?;

        let mut tabs = self.tabs.write().expect("TabManager: tabs write lock poisoned");
        tabs.order.push(tab_id.clone());
        tabs.map.insert(tab_id.clone(), Arc::new(tab_controller));
        log::info!("TabManager: Created lazy tab {}", tab_id);
        Ok(tab_id)
    }

    fn destroy_tab(&self, tab_id: &str) -> tauri::Result<()> {
        let (removed, next) = {
            let mut tabs = self.tabs.write().expect("TabManager: tabs write lock poisoned");
            let Some(index) = tabs.order.iter().position(|id| id == tab_id) else {
                return Ok(());
            };
            tabs.order.remove(index);
            let removed = tabs.map.remove(tab_id);
            if let Some(tab) = removed.as_ref() {
                let url = tab.current_url();
                tabs.closed.push(url);
                if tabs.closed.len() > CLOSED_TAB_HISTORY {
                    tabs.closed.remove(0);
                }
            }
            // Like browsers: activate the tab to the right, else to the left.
            let next = if tabs.active.as_deref() == Some(tab_id) {
                tabs.active = None;
                tabs.order
                    .get(index)
                    .or_else(|| tabs.order.get(index.wrapping_sub(1)))
                    .cloned()
            } else {
                None
            };
            (removed, next)
        };
        if let Some(tab) = removed {
            tab.destroy()?;
        }
        if let Some(next) = next {
            self.show_tab(&next)?;
        }
        Ok(())
    }

    fn show_tab(&self, tab_id: &str) -> tauri::Result<()> {
        let (target, previous) = {
            let mut tabs = self.tabs.write().expect("TabManager: tabs write lock poisoned");
            let Some(target) = tabs.map.get(tab_id).cloned() else {
                return Ok(());
            };
            let previous = tabs
                .active
                .replace(tab_id.to_string())
                .filter(|prev| prev != tab_id)
                .and_then(|prev| tabs.map.get(&prev).cloned());
            (target, previous)
        };
        // Show first, then hide, so the window never flashes empty.
        target.show()?;
        if let Some(previous) = previous {
            previous.hide()?;
        }
        Ok(())
    }

    fn get_tab_ids(&self) -> Vec<String> {
        self.tabs
            .read()
            .expect("TabManager: tabs read lock poisoned")
            .order
            .clone()
    }

    fn active_tab_id(&self) -> Option<String> {
        self.tabs
            .read()
            .expect("TabManager: tabs read lock poisoned")
            .active
            .clone()
    }

    fn active_webview(&self) -> Option<tauri::Webview<R>> {
        let id = self.active_tab_id()?;
        self.get(&id).map(|t| t.webview.clone())
    }

    fn tab_states(&self) -> Vec<crate::state::TabState> {
        let active = self.active_tab_id();
        self.all()
            .into_iter()
            .map(|tab| crate::state::TabState {
                id: tab.tab_id.clone(),
                title: tab.current_title(),
                url: tab.current_url(),
                is_active: active.as_deref() == Some(tab.tab_id.as_str()),
                is_pinned: false,
            })
            .collect()
    }

    fn tab_url(&self, tab_id: &str) -> Option<String> {
        self.get(tab_id).map(|t| t.current_url())
    }

    fn update_tab_meta(&self, tab_id: &str, title: Option<String>, url: Option<String>) -> bool {
        let Some(tab) = self.get(tab_id) else {
            return false;
        };
        let mut changed = false;
        if let Some(title) = title {
            let mut current = tab.title.lock().expect("TabController title lock poisoned");
            if *current != title {
                *current = title;
                changed = true;
            }
        }
        if let Some(url) = url {
            let mut current = tab.url.lock().expect("TabController url lock poisoned");
            if *current != url {
                *current = url;
                changed = true;
            }
        }
        changed
    }

    fn pop_closed_tab(&self) -> Option<String> {
        self.tabs
            .write()
            .expect("TabManager: tabs write lock poisoned")
            .closed
            .pop()
    }

    fn move_tab(&self, tab_id: &str, to_index: usize) {
        let mut tabs = self.tabs.write().expect("TabManager: tabs write lock poisoned");
        if let Some(from) = tabs.order.iter().position(|id| id == tab_id) {
            let id = tabs.order.remove(from);
            let to = to_index.min(tabs.order.len());
            tabs.order.insert(to, id);
        }
    }

    fn layout(&self, window: &tauri::Window<R>) {
        let Ok((position, size)) = crate::window_controller::content_bounds(window) else {
            return;
        };
        let active = self.active_tab_id();
        for tab in self.all() {
            if tab.window_id != window.label() {
                continue;
            }
            if active.as_deref() == Some(tab.tab_id.as_str()) {
                let _ = tab.webview.set_bounds(tauri::Rect {
                    position: position.into(),
                    size: size.into(),
                });
            } else {
                tab.needs_layout.store(true, Ordering::Release);
            }
        }
    }

    fn suspend_idle_tabs(&self, max_idle: Duration) {
        let active = self.active_tab_id();
        let now = Instant::now();
        let mut candidates: Vec<(Duration, Arc<TabController<R>>)> = self
            .all()
            .into_iter()
            .filter(|tab| active.as_deref() != Some(tab.tab_id.as_str()))
            .map(|tab| {
                let idle = now.duration_since(*tab.last_active.lock().expect("lock poisoned"));
                (idle, tab)
            })
            .collect();
        // Most recently used first; those (plus the active tab) stay loaded.
        candidates.sort_by_key(|(idle, _)| *idle);
        for (idle, tab) in candidates.into_iter().skip(KEEP_RECENT_TABS.saturating_sub(1)) {
            if idle >= max_idle && !tab.suspended.load(Ordering::Acquire) {
                let _ = tab.suspend();
            }
        }
    }

    fn inject_theme_into_tab(
        &self,
        app: &AppHandle<R>,
        tab_id: &str,
        theme_name: &str,
    ) -> tauri::Result<()> {
        if let Some(tab) = self.get(tab_id) {
            let theming = app.state::<Arc<dyn crate::traits::ThemingEngine<R>>>();
            theming.inject_theme(&tab.webview, theme_name);
        }
        Ok(())
    }
}
