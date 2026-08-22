#[tokio::test]
#[ignore = "manual diagnostic probe requiring a dedicated PostgreSQL database"]
async fn probe_search_rebuild_root_cause() -> Result<(), Box<dyn std::error::Error>> {
    let database_url = std::env::var("DEBUG_DB_URL")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let config = melodee_search::config::SearchConfig {
        search_root: "/tmp/r4_debug2/search".into(),
        ..Default::default()
    };
    let job = melodee_search::request_rebuild(&config, &pool, "probe").await?;
    println!("requested: {}", job.job_id);
    let result = melodee_search::run_rebuild_once(&config, &pool).await;
    println!("rebuild result: {result:?}");
    Ok(())
}
