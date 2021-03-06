// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![deny(missing_docs)]

//! Glean is a modern approach for recording and sending Telemetry data.
//!
//! It's in use at Mozilla.
//!
//! All documentation can be found online:
//!
//! ## [The Glean SDK Book](https://mozilla.github.io/glean)
//!
//! ## Example
//!
//! Initialize Glean, register a ping and then send it.
//!
//! ```rust,no_run
//! # use glean::{Configuration, ClientInfoMetrics, Error, private::*};
//! let cfg = Configuration {
//!     data_path: "/tmp/data".into(),
//!     application_id: "org.mozilla.glean_core.example".into(),
//!     upload_enabled: true,
//!     max_events: None,
//!     delay_ping_lifetime_io: false,
//!     channel: None,
//! };
//! glean::initialize(cfg, ClientInfoMetrics::unknown());
//!
//! let prototype_ping = PingType::new("prototype", true, true, vec!());
//!
//! glean::register_ping_type(&prototype_ping);
//!
//! prototype_ping.submit(None);
//! ```

use once_cell::sync::OnceCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub use configuration::Configuration;
pub use core_metrics::ClientInfoMetrics;
pub use glean_core::{global_glean, setup_glean, CommonMetricData, Error, Glean, Lifetime, Result};

mod configuration;
mod core_metrics;
pub mod dispatcher;
mod glean_metrics;
pub mod private;
mod system;

const LANGUAGE_BINDING_NAME: &str = "Rust";

/// State to keep track for the Rust Language bindings.
///
/// This is useful for setting Glean SDK-owned metrics when
/// the state of the upload is toggled.
#[derive(Debug)]
struct RustBindingsState {
    /// The channel the application is being distributed on.
    channel: Option<String>,

    /// Client info metrics set by the application.
    client_info: ClientInfoMetrics,
}

/// Set when `glean::initialize()` returns.
/// This allows to detect calls that happen before `glean::initialize()` was called.
/// Note: The initialization might still be in progress, as it runs in a separate thread.
static INITIALIZE_CALLED: AtomicBool = AtomicBool::new(false);

/// A global singleton storing additional state for Glean.
///
/// Requires a Mutex, because in tests we can actual reset this.
static STATE: OnceCell<Mutex<RustBindingsState>> = OnceCell::new();

/// Get a reference to the global state object.
///
/// Panics if no global state object was set.
fn global_state() -> &'static Mutex<RustBindingsState> {
    STATE.get().unwrap()
}

/// Set or replace the global Glean object.
fn setup_state(state: RustBindingsState) {
    if STATE.get().is_none() {
        STATE.set(Mutex::new(state)).unwrap();
    } else {
        let mut lock = STATE.get().unwrap().lock().unwrap();
        *lock = state;
    }
}

fn with_glean<F, R>(f: F) -> R
where
    F: Fn(&Glean) -> R,
{
    let glean = global_glean().expect("Global Glean object not initialized");
    let lock = glean.lock().unwrap();
    f(&lock)
}

fn with_glean_mut<F, R>(f: F) -> R
where
    F: Fn(&mut Glean) -> R,
{
    let glean = global_glean().expect("Global Glean object not initialized");
    let mut lock = glean.lock().unwrap();
    f(&mut lock)
}

/// Creates and initializes a new Glean object.
///
/// See `glean_core::Glean::new` for more information.
///
/// # Arguments
///
/// * `cfg` - the `Configuration` options to initialize with.
/// * `client_info` - the `ClientInfoMetrics` values used to set Glean
///   core metrics.
pub fn initialize(cfg: Configuration, client_info: ClientInfoMetrics) {
    if was_initialize_called() {
        log::error!("Glean should not be initialized multiple times");
        return;
    }

    std::thread::spawn(move || {
        let core_cfg = glean_core::Configuration {
            upload_enabled: cfg.upload_enabled,
            data_path: cfg.data_path.clone(),
            application_id: cfg.application_id.clone(),
            language_binding_name: LANGUAGE_BINDING_NAME.into(),
            max_events: cfg.max_events,
            delay_ping_lifetime_io: cfg.delay_ping_lifetime_io,
        };

        let glean = match Glean::new(core_cfg) {
            Ok(glean) => glean,
            // glean-core already takes care of logging errors: other bindings
            // simply do early returns, as we're doing.
            Err(_) => return,
        };

        // glean-core already takes care of logging errors: other bindings
        // simply do early returns, as we're doing.
        if glean_core::setup_glean(glean).is_err() {
            return;
        }

        log::info!("Glean initialized");

        // Now make this the global object available to others.
        setup_state(RustBindingsState {
            channel: cfg.channel,
            client_info,
        });

        let upload_enabled = cfg.upload_enabled;

        with_glean_mut(|glean| {
            let state = global_state().lock().unwrap();

            // Get the current value of the dirty flag so we know whether to
            // send a dirty startup baseline ping below.  Immediately set it to
            // `false` so that dirty startup pings won't be sent if Glean
            // initialization does not complete successfully.
            // TODO Bug 1672956 will decide where to set this flag again.
            let dirty_flag = glean.is_dirty_flag_set();
            glean.set_dirty_flag(false);

            // Register builtin pings.
            // Unfortunately we need to manually list them here to guarantee
            // they are registered synchronously before we need them.
            // We don't need to handle the deletion-request ping. It's never touched
            // from the language implementation.
            glean.register_ping_type(&glean_metrics::pings::baseline.ping_type);
            glean.register_ping_type(&glean_metrics::pings::metrics.ping_type);
            glean.register_ping_type(&glean_metrics::pings::events.ping_type);

            // TODO: perform registration of pings that were attempted to be
            // registered before init. See bug 1673850.

            // If this is the first time ever the Glean SDK runs, make sure to set
            // some initial core metrics in case we need to generate early pings.
            // The next times we start, we would have them around already.
            let is_first_run = glean.is_first_run();
            if is_first_run {
                initialize_core_metrics(&glean, &state.client_info, state.channel.clone());
            }

            // Deal with any pending events so we can start recording new ones
            let pings_submitted = glean.on_ready_to_submit_pings();

            // We need to kick off upload in these cases:
            // 1. Pings were submitted through Glean and it is ready to upload those pings;
            // 2. Upload is disabled, to upload a possible deletion-request ping.
            if pings_submitted || !upload_enabled {
                // TODO: bug 1672958.
            }

            // Set up information and scheduling for Glean owned pings. Ideally, the "metrics"
            // ping startup check should be performed before any other ping, since it relies
            // on being dispatched to the API context before any other metric.
            // TODO: start the metrics ping scheduler, will happen in bug 1672951.

            // Check if the "dirty flag" is set. That means the product was probably
            // force-closed. If that's the case, submit a 'baseline' ping with the
            // reason "dirty_startup". We only do that from the second run.
            if !is_first_run && dirty_flag {
                // TODO: bug 1672958 - submit_ping_by_name_sync("baseline", "dirty_startup");
            }

            // From the second time we run, after all startup pings are generated,
            // make sure to clear `lifetime: application` metrics and set them again.
            // Any new value will be sent in newly generated pings after startup.
            if !is_first_run {
                glean.clear_application_lifetime_metrics();
                initialize_core_metrics(&glean, &state.client_info, state.channel.clone());
            }
        });

        // Signal Dispatcher that init is complete
        if let Err(err) = dispatcher::flush_init() {
            log::error!("Unable to flush the preinit queue: {}", err);
        }
    });

    // Mark the initialization as called: this needs to happen outside of the
    // dispatched block!
    INITIALIZE_CALLED.store(true, Ordering::SeqCst);
}

/// Checks if `glean::initialize` was ever called.
///
/// # Returns
///
/// `true` if it was, `false` otherwise.
fn was_initialize_called() -> bool {
    INITIALIZE_CALLED.load(Ordering::SeqCst)
}

fn initialize_core_metrics(
    glean: &Glean,
    client_info: &ClientInfoMetrics,
    channel: Option<String>,
) {
    let core_metrics = core_metrics::InternalMetrics::new();

    core_metrics
        .app_build
        .set(glean, &client_info.app_build[..]);
    core_metrics
        .app_display_version
        .set(glean, &client_info.app_display_version[..]);
    if let Some(app_channel) = channel {
        core_metrics.app_channel.set(glean, app_channel);
    }
    core_metrics.os_version.set(glean, "unknown".to_string());
    core_metrics
        .architecture
        .set(glean, system::ARCH.to_string());
    core_metrics
        .device_manufacturer
        .set(glean, "unknown".to_string());
    core_metrics.device_model.set(glean, "unknown".to_string());
}

/// Sets whether upload is enabled or not.
///
/// See `glean_core::Glean.set_upload_enabled`.
pub fn set_upload_enabled(enabled: bool) {
    if !was_initialize_called() {
        let msg =
            "Changing upload enabled before Glean is initialized is not supported.\n \
            Pass the correct state into `Glean.initialize()`.\n \
            See documentation at https://mozilla.github.io/glean/book/user/general-api.html#initializing-the-glean-sdk";
        log::error!("{}", msg);
        return;
    }

    // Changing upload enabled always happens asynchronous.
    // That way it follows what a user expect when calling it inbetween other calls:
    // it executes in the right order.
    //
    // Because the dispatch queue is halted until Glean is fully initialized
    // we can safely enqueue here and it will execute after initialization.
    dispatcher::launch(move || {
        with_glean_mut(|glean| {
            let state = global_state().lock().unwrap();
            let old_enabled = glean.is_upload_enabled();
            glean.set_upload_enabled(enabled);

            // TODO: Cancel upload and any outstanding metrics ping scheduler
            // task. Will happen on bug 1672951.

            if !old_enabled && enabled {
                // If uploading is being re-enabled, we have to restore the
                // application-lifetime metrics.
                initialize_core_metrics(&glean, &state.client_info, state.channel.clone());
            }

            // TODO: trigger upload for the deletion-ping. Will happen in bug 1672952.
        });
    });
}

/// Register a new [`PingType`](metrics/struct.PingType.html).
pub fn register_ping_type(ping: &private::PingType) {
    let ping = ping.clone();
    dispatcher::launch(move || {
        with_glean_mut(|glean| {
            glean.register_ping_type(&ping.ping_type);
        })
    })
}

/// Collects and submits a ping for eventual uploading.
///
/// See `glean_core::Glean.submit_ping`.
pub fn submit_ping(ping: &private::PingType, reason: Option<&str>) {
    submit_ping_by_name(&ping.name, reason)
}

/// Collects and submits a ping for eventual uploading by name.
///
/// See `glean_core::Glean.submit_ping_by_name`.
pub fn submit_ping_by_name(ping: &str, reason: Option<&str>) {
    let ping = ping.to_string();
    let reason = reason.map(|s| s.to_string());
    dispatcher::launch(move || {
        with_glean(|glean| glean.submit_ping_by_name(&ping, reason.as_deref()).ok());
    })
}

#[cfg(test)]
mod test;
