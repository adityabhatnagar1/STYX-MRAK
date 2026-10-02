//! ϟ МРАК — process entry point: config, database, Discord client, feed
//! worker and graceful shutdown.

use anyhow::Context as _;
use serenity::all::{Client, GatewayIntents};
use std::sync::Arc;
use styx_mrak::config::Config;
use styx_mrak::db;
use styx_mrak::discord::actions::DiscordAnnouncer;
use styx_mrak::discord::handler::Handler;
use styx_mrak::feeds::service::run_worker;
use styx_mrak::feeds::source::{FeedSource, XFeedSource};
use styx_mrak::state::AppState;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")))
        .init();

    let cfg = Config::load().context("invalid configuration (see .env.example)")?;
    tracing::info!(config = ?cfg, "configuration loaded");

    let pool = db::connect(&cfg.database_url).await.context("database")?;
    tracing::info!("database connected, migrations applied");

    let state = AppState::new(cfg.clone(), pool.clone());

    // Only request the privileged message-content intent when automod needs it.
    let mut intents = GatewayIntents::GUILDS;
    if cfg.automod.enabled {
        intents |= GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;
    }

    let mut client = Client::builder(&cfg.discord_token, intents)
        .event_handler(Handler::new(state.clone()))
        .await
        .context("could not build the Discord client")?;
    state.set_http(client.http.clone());

    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    let feed_task = if cfg.feed.enabled {
        let token = cfg.feed.x_bearer_token.clone().unwrap_or_default();
        let source: Arc<dyn FeedSource> =
            Arc::new(XFeedSource::new(&cfg.feed.x_api_base, &token, &cfg.feed.x_username).context("feed source")?);
        let announcer = Arc::new(DiscordAnnouncer { state: state.clone() });
        Some(tokio::spawn(run_worker(
            pool.clone(),
            source,
            announcer,
            cfg.feed.clone(),
            state.feed_status.clone(),
            shutdown_rx,
        )))
    } else {
        tracing::info!("feed disabled (FEED_ENABLED=false)");
        None
    };

    let shard_manager = client.shard_manager.clone();
    let signal_tx = shutdown_tx.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutdown signal received");
        let _ = signal_tx.send(true);
        shard_manager.shutdown_all().await;
    });

    let result = client.start().await;
    if let Err(e) = &result {
        tracing::error!(error = %e, "discord client stopped with an error");
    }

    // Orderly teardown: stop the worker, then close the database.
    let _ = shutdown_tx.send(true);
    if let Some(t) = feed_task {
        let _ = t.await;
    }
    pool.close().await;
    tracing::info!("shutdown complete");
    result.map_err(Into::into)
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
