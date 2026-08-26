//! Bounded chart CSV import command.

use std::path::PathBuf;

use clap::Subcommand;
use melodee_persistence::{MAX_CHART_CSV_BYTES, NativeChartImport, NativeDiscoveryStore};

use super::common::{parse_nonblank, pool, read_bounded_file};

#[derive(Debug, Subcommand)]
pub(crate) enum ChartCommands {
    /// Atomically create a visible chart from Rank,ArtistName,ReleaseTitle,ReleaseYear CSV rows.
    Import {
        #[arg(long)]
        file: PathBuf,
        #[arg(long, value_parser = parse_nonblank)]
        slug: String,
        #[arg(long, value_parser = parse_nonblank)]
        title: String,
        #[arg(long)]
        source_name: Option<String>,
        #[arg(long)]
        source_url: Option<String>,
        #[arg(long)]
        year: Option<i32>,
        #[arg(long, default_value_t = false)]
        hidden: bool,
        #[arg(long, default_value_t = false)]
        generated_playlist: bool,
    },
}

pub(crate) async fn run(command: ChartCommands) {
    match command {
        ChartCommands::Import {
            file,
            slug,
            title,
            source_name,
            source_url,
            year,
            hidden,
            generated_playlist,
        } => {
            let input = read_bounded_file(&file, MAX_CHART_CSV_BYTES).unwrap_or_else(|error| {
                eprintln!("error: cannot open chart CSV {}: {error}", file.display());
                std::process::exit(2);
            });
            if input.len() > MAX_CHART_CSV_BYTES {
                eprintln!("error: chart CSV exceeds {MAX_CHART_CSV_BYTES} bytes");
                std::process::exit(2);
            }
            let pool = pool().await;
            melodee_persistence::migrate(&pool)
                .await
                .unwrap_or_else(|error| {
                    eprintln!("error: migration failed: {error}");
                    std::process::exit(1);
                });
            let result = NativeDiscoveryStore
                .import_chart_csv(
                    &pool,
                    &NativeChartImport {
                        slug,
                        title,
                        source_name,
                        source_url,
                        chart_year: year,
                        is_visible: !hidden,
                        is_generated_playlist_enabled: generated_playlist,
                    },
                    &input,
                )
                .await
                .unwrap_or_else(|error| {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                });
            println!("chart_id:    {}", result.chart_id);
            println!("items:       {}", result.item_count);
            println!("linked:      {}", result.linked_count);
            println!("unlinked:    {}", result.item_count - result.linked_count);
        }
    }
}
