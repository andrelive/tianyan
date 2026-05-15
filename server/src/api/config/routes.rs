use std::sync::Arc;

use axum::{routing::get, Router};

use crate::api::config::handlers::{
    get_config, get_config_section, get_models, switch_model, update_config,
};
use crate::state::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config", get(get_config).put(update_config))
        .route("/config/{section}", get(get_config_section))
        .route("/config/models", get(get_models))
        .route("/config/models/switch", axum::routing::post(switch_model))
}
