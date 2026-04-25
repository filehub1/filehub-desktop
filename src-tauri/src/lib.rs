pub mod config;
pub mod indexer;
pub mod preview;
pub mod server;

use std::net::TcpListener;
use std::sync::{Arc, RwLock};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

use config::load_config;
use indexer::FileIndex;
use server::{start_server, AppState};

fn find_free_port(start: u16) -> u16 {
    (start..65535)
        .find(|&p| TcpListener::bind(("127.0.0.1", p)).is_ok())
        .unwrap_or(start)
}

/// Start a TCP proxy on 0.0.0.0 that forwards to 127.0.0.1:main_port.
/// Returns the LAN port it bound to.
async fn start_lan_proxy(main_port: u16) -> u16 {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
    let lan_port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let Ok(mut server) = tokio::net::TcpStream::connect(("127.0.0.1", main_port)).await else { return };
                let (mut cr, mut cw) = client.split();
                let (mut sr, mut sw) = server.split();
                tokio::select! {
                    _ = tokio::io::copy(&mut cr, &mut sw) => {}
                    _ = tokio::io::copy(&mut sr, &mut cw) => {}
                }
            });
        }
    });
    lan_port
}

fn static_dir(_app: &tauri::AppHandle) -> std::path::PathBuf {
    // 1. resource_dir/dist (安装后，tauri 打包的资源)
    if let Ok(res) = _app.path().resource_dir() {
        let candidate = res.join("dist");
        if candidate.join("index.html").exists() {
            return candidate;
        }
    }
    // 2. 便携版: exe 同级的 dist/
    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.parent().unwrap().join("dist");
        if candidate.join("index.html").exists() {
            return candidate;
        }
    }
    // 3. 开发模式: src-tauri/dist
    if let Ok(exe) = std::env::current_exe() {
        if let Some(src_tauri) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let candidate = src_tauri.join("dist");
            if candidate.join("index.html").exists() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from("dist")
}

pub fn run() {
    tracing_subscriber::fmt::init();

    let port = find_free_port(6543);
    let cfg = load_config();
    let index = Arc::new(FileIndex::new(
        cfg.indexed_directories.clone(),
        cfg.exclude_patterns.clone(),
    ));
    index.rebuild();

    let state = AppState {
        config: Arc::new(RwLock::new(cfg)),
        index,
        lan_port: Arc::new(RwLock::new(None)),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .on_page_load(|window, _payload| {
            let _ = window.eval("window.__FILEHUB_IS_LOCAL__ = true;");
        })
        .setup(move |app| {
            let static_path = static_dir(app.handle());
            let state_clone = state.clone();

            // Start axum server in background (only for API, not for HTML)
            tauri::async_runtime::spawn(async move {
                let lan_enabled = state_clone.config.read().unwrap().lan_enabled;
                if lan_enabled {
                    let lp = start_lan_proxy(port).await;
                    *state_clone.lan_port.write().unwrap() = Some(lp);
                    tracing::info!("LAN proxy on port {}", lp);
                }
                start_server(state_clone, port, static_path).await;
            });

            // Poll until the port is actually listening (up to 5s)
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if TcpListener::bind(("127.0.0.1", port)).is_err() {
                    break; // port is taken → server is up
                }
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }

            // Load the app from the local axum server so the page and /api calls
            // share the same origin in both dev and packaged builds.
            let url = format!("http://127.0.0.1:{port}");
            let win = app.get_webview_window("main").expect("main window missing");
            win.navigate(url.parse().expect("invalid url"))?;

            // Tray
            let quit = MenuItem::with_id(app, "quit", "Quit FileHub", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;

            TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("FileHub")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => app.exit(0),
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                // Close the app instead of hiding to tray, so Windows does not keep
                // a leftover background process after the main window is closed.
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
