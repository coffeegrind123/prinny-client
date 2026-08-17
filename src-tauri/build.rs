use tauri_build::{AppManifest, Attributes, DefaultPermissionRule, InlinedPlugin};

/// Every command in `generate_handler!`, i.e. everything the frontend can call
/// that is not part of a plugin.
///
/// These have to be listed here or they are **unreachable from the page**, and
/// they were: `tauri_build::build()` generates no app manifest, so no capability
/// could grant an app command, so `resolve_access` returned `None` for all of
/// them and every `invoke()` was rejected before reaching Rust.
///
/// The reason that was invisible is that the ACL check only applies to a
/// command when the caller is a plugin command, the app has a manifest, or the
/// origin is remote (tauri 2.11.5, `webview/mod.rs:1819`) — and this app is
/// always the third case. `tauri-plugin-localhost` serves the frontend on
/// `http://localhost:44548`, which `is_local_url` classifies as remote because
/// it is neither the tauri protocol nor relative to `frontendDist`. So the gate
/// that usually only bites hosted content bites the app's own UI here, on every
/// platform, and the failure surfaced as features quietly doing nothing:
/// notification avatars, the taskbar badge, drag-and-drop file reads, link
/// previews, rich presence.
///
/// Adding a command to `generate_handler!` and not to this list re-breaks it in
/// exactly that silent way. Keep them in step.
const APP_COMMANDS: &[&str] = &[
    "set_badge_count",
    "set_homeserver_origin",
    "cache_notification_icon",
    "read_dropped_file",
    "fetch_remote_bytes",
    "fetch_og_preview",
    "probe_push_gateway",
    "send_windows_message_toast",
    "arm_capture_intent",
    "set_capture_session",
    "set_content_protection",
    "start_rich_presence_bridge",
    "stop_rich_presence_bridge",
];

/// The Android plugins declared inline in `lib.rs` with
/// `tauri::plugin::Builder::new(..).setup(register_android_plugin)`.
///
/// A plugin registered that way has no permission manifest of its own, which is
/// what `InlinedPlugin` is for. Without it the same ACL gate above rejected
/// every call into them — which is why Android had no push registration, no
/// foreground service, no custom notification and no share target: the Kotlin
/// behind all four was never reached.
///
/// Command names are written exactly as the frontend invokes them. Tauri
/// lower-camel-cases the command on its way to Kotlin (`webview/mod.rs`
/// `AsLowerCamelCase`), so `get_endpoint` here is `getEndpoint` there, but the
/// ACL matches the string the page sent.
///
/// `registerListener`/`removeListener` are Tauri's own plugin commands, used by
/// `addPluginListener()`. They are already camelCase on the wire, and they are
/// how a Kotlin `trigger()` reaches JS at all — leaving them out would allow the
/// commands but silently drop every event.
struct AndroidPlugin {
    name: &'static str,
    commands: &'static [&'static str],
}

const ANDROID_PLUGINS: &[AndroidPlugin] = &[
    AndroidPlugin {
        name: "unifiedpush",
        commands: &[
            "register",
            "get_endpoint",
            "get_distributors",
            "get_status",
            "registerListener",
            "removeListener",
        ],
    },
    AndroidPlugin {
        name: "foreground",
        commands: &[
            "start_foreground",
            "stop_foreground",
            "is_foreground_running",
            "set_microphone_active",
        ],
    },
    AndroidPlugin {
        name: "message-notification",
        commands: &["show", "js_ready", "registerListener", "removeListener"],
    },
    AndroidPlugin {
        name: "share-target",
        commands: &[
            "js_ready",
            "read_shared_file",
            "registerListener",
            "removeListener",
        ],
    },
];

fn main() {
    let mut attributes = Attributes::new().app_manifest(AppManifest::new().commands(APP_COMMANDS));

    for plugin in ANDROID_PLUGINS {
        attributes = attributes.plugin(
            plugin.name,
            InlinedPlugin::new()
                .commands(plugin.commands)
                .default_permission(DefaultPermissionRule::AllowAllCommands),
        );
    }

    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
