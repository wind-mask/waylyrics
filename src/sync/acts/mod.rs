use crate::{
    app::search_window,
    log::{info, warn},
    sync::LyricState,
    utils::bind_shortcut,
};
use glib_macros::clone;
use gtk::{
    gio::SimpleAction,
    glib::{self, VariantTy},
    prelude::*,
    subclass::prelude::ObjectSubclassIsExt,
    Application,
};

use crate::{
    app::{self, dialog::show_dialog},
    glib_spawn,
    lyric_providers::LyricOwned,
    sync::{
        interop::clean_player, interop::common::update_lyric, TrackState, LYRIC,
        TRACK_PLAYING_STATE,
    },
    utils::reset_lyric_labels,
    MAIN_WINDOW,
};

use crate::sync::interop::connect_player_with_id;
mod utils;

pub fn register_disconnect(app: &Application) {
    let action = SimpleAction::new("disconnect", None);
    action.connect_activate(|_, _| {
        clean_player();
    });
    app.add_action(&action);
}

pub fn register_search_lyric(app: &Application, wind: &app::Window, trigger: &str) {
    let action = SimpleAction::new("search-lyric", None);
    let cache_lyrics = wind.imp().cache_lyrics.get();
    action.connect_activate(move |_, _| {
        // Get current playing track
        let (title, album, artists) =
            TRACK_PLAYING_STATE.with_borrow(|TrackState { metainfo, .. }| {
                let Some(track) = metainfo.as_ref() else {
                    return Default::default();
                };
                let artists = track
                    .artists
                    .as_ref()
                    .map(|artists| {
                        artists
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<&str>>()
                            .join("/")
                    })
                    .unwrap_or_default();
                let title = track.title.as_deref().unwrap_or_default().to_string();
                let album = track.album.as_deref().unwrap_or_default().to_string();
                (title, album, artists)
            });

        let window = search_window::Window::new(title, album, artists, cache_lyrics);
        window.present();
    });
    app.add_action(&action);

    bind_shortcut("app.search-lyric", wind, trigger);
}

/// update lyric, but do not ignore cache
pub fn register_reload_lyric(app: &Application) {
    let action = SimpleAction::new("reload-lyric", None);
    action.connect_activate(move |_, _| {
        let metainfo = TRACK_PLAYING_STATE
            .with_borrow(|TrackState { metainfo, .. }| metainfo.as_ref().cloned());
        let Some(metainfo) = metainfo else {
            return;
        };

        crate::log::debug!("spawned update_lyric from reload-lyric action");
        glib_spawn!(async move {
            let Some(wind) = MAIN_WINDOW.with_borrow(|wind| wind.as_ref().cloned()) else {
                return;
            };
            reset_lyric_labels(&wind, None);
            if let Err(err) = update_lyric(&metainfo, &wind, false).await {
                show_dialog(
                    Some(&wind),
                    &format!("cannot refetch lyric: {err:?}"),
                    gtk::MessageType::Error,
                );
            }
        });
    });
    app.add_action(&action);
}

pub fn register_refetch_lyric(app: &Application, window: &app::Window, trigger: &str) {
    let action = SimpleAction::new("refetch-lyric", None);
    action.connect_activate(move |_, _| {
        let metainfo = TRACK_PLAYING_STATE
            .with_borrow(|TrackState { metainfo, .. }| metainfo.as_ref().cloned());
        let Some(metainfo) = metainfo else {
            return;
        };

        crate::log::debug!("spawned update_lyric from refetch-lyric action");
        glib_spawn!(async move {
            let Some(wind) = MAIN_WINDOW.with_borrow(|wind| wind.as_ref().cloned()) else {
                return;
            };
            reset_lyric_labels(&wind, None);
            if let Err(err) = update_lyric(&metainfo, &wind, true).await {
                show_dialog(
                    Some(&wind),
                    &format!("cannot refetch lyric: {err:?}"),
                    gtk::MessageType::Error,
                );
            }
        });
    });
    app.add_action(&action);

    bind_shortcut("app.refetch-lyric", window, trigger);
}

pub fn register_remove_lyric(app: &Application, wind: &app::Window) {
    let action = SimpleAction::new("remove-lyric", None);
    action.connect_activate(clone!(@weak wind as window => move |_, _| {
        // Clear current lyric
        let origin = LyricOwned::LineTimestamp(vec![]);
        let translation = LyricOwned::None;
        LYRIC.set(LyricState{ origin, translation });
        let cache_lyrics = window.imp().cache_lyrics.get();
        // Update cache
        if cache_lyrics {
            utils::update_cache();
        }
        // Remove current lyric inside window
        reset_lyric_labels(&window, None);
        info!("removed lyric");
    }));
    app.add_action(&action);
}

#[cfg(feature = "import-lrc")]
pub fn register_import_original_lyric(app: &Application, wind: &app::Window) {
    use crate::log::error;
    use crate::lyric_providers::{utils::lrc_iter, Lyric};

    let action = SimpleAction::new("import-original-lyric", None);
    action.connect_activate(clone!(@weak wind as window => move |_, _| {
        glib_spawn!(async move {
            let lrc_file = rfd::AsyncFileDialog::new().add_filter("Simple LRC", &["lrc"]).pick_file().await;
            let Some(lrc_file) = lrc_file else {
                return;
            };
            let lrc = match String::from_utf8(lrc_file.read().await) {
                Ok(lrc) => lrc,
                Err(e) => {
                    let error_msg = format!( "failed to read LRC in UTF-8: {e}");
                    error!(error_msg);
                    show_dialog(gtk::Window::NONE, &error_msg, gtk::MessageType::Error);
                    return;
                }
            };
            if let Ok(lyric) = lrc_iter(lrc.lines()) {
                LYRIC.with_borrow_mut(|LyricState { origin, .. }|{
                    *origin = Lyric::LineTimestamp(lyric).into_owned();
                });
            }
            let cache_lyrics = window.imp().cache_lyrics.get();
            if cache_lyrics {
                utils::update_cache();
            }
        });
    }));
    app.add_action(&action);
}

#[cfg(feature = "import-lrc")]
pub fn register_import_translated_lyric(app: &Application, wind: &app::Window) {
    use crate::log::error;
    use crate::lyric_providers::{utils::lrc_iter, Lyric};

    let action = SimpleAction::new("import-translated-lyric", None);
    action.connect_activate(clone!(@weak wind as window => move |_, _| {
        glib_spawn!(async move {
            let lrc_file = rfd::AsyncFileDialog::new().add_filter("Simple LRC", &["lrc"]).pick_file().await;
            let Some(lrc_file) = lrc_file else {
                return;
            };
            let lrc = match String::from_utf8(lrc_file.read().await) {
                Ok(lrc) => lrc,
                Err(e) => {
                    let error_msg = format!( "failed to read LRC in UTF-8: {e}");
                    error!(error_msg);
                    show_dialog(gtk::Window::NONE, &error_msg, gtk::MessageType::Error);
                    return;
                }
            };
            if let Ok(lyric) = lrc_iter(lrc.lines()) {
                LYRIC.with_borrow_mut(|LyricState { translation, .. }|{
                    *translation = Lyric::LineTimestamp(lyric).into_owned();
                });
            }
            let cache_lyrics = window.imp().cache_lyrics.get();
            if cache_lyrics {
                utils::update_cache();
            }
        });
    }));
    app.add_action(&action);
}

pub fn register_connect(app: &Application) {
    let connect = SimpleAction::new("connect", Some(VariantTy::STRING));
    connect.connect_activate(|_, player_id| {
        let Some(player_id) = player_id.and_then(|p| p.str()) else {
            warn!("did not received string paramter for action \'app.connect\'");
            return;
        };

        connect_player_with_id(player_id)
    });
    app.add_action(&connect);
}

#[cfg(feature = "action-event")]
mod event;
#[cfg(feature = "action-event")]
pub use event::{init_play_action_channel, PlayAction, PLAY_ACTION};
