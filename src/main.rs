use futures_util::TryStreamExt;
use octocrab::params::{pulls::Sort, Direction, State};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use tokio::pin;
use tracing::{debug, info};

mod database;

const LAST_PR_ID_FILE: &str = "data/last_pr_id";
const LOGS_DIR: &str = "data/logs";
const REPO_DIR: &str = "data/repo";
const MAIN_BRANCH: &str = "master";
const GH_POLL_INTERVAL: u64 = 60;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let personal_token = std::env::var("BOT_TOKEN").expect("BOT_TOKEN is required env");
    let upstream_owner = std::env::var("UPSTREAM_OWNER").expect("UPSTREAM_OWNER is required env");
    let upstream_repo = std::env::var("UPSTREAM_REPO").expect("UPSTREAM_REPO is required env");
    let downstream_owner =
        std::env::var("DOWNSTREAM_OWNER").expect("DOWNSTREAM_OWNER is required env");
    let downstream_repo =
        std::env::var("DOWNSTREAM_REPO").expect("DOWNSTREAM_REPO is required env");

    if let Err(e) = std::fs::create_dir("data") {
        if e.kind() != std::io::ErrorKind::AlreadyExists {
            panic!("failed to create data directory - {}", e)
        }
    }

    let mut last_mirrored_pr_id = if Path::new(LAST_PR_ID_FILE).exists() {
        let mut value = String::new();
        File::open(LAST_PR_ID_FILE)
            .expect("failed to open file with saved data")
            .read_to_string(&mut value)
            .unwrap();
        value.parse::<u64>().unwrap()
    } else {
        let id = std::env::var("START_PR_ID")
            .expect("Set START_PR_ID to initialize parsing")
            .parse::<u64>()
            .unwrap();
        std::fs::write(LAST_PR_ID_FILE, id.to_string()).unwrap();
        id
    };
    debug!("last PR id: {}", last_mirrored_pr_id);

    let crab = octocrab::OctocrabBuilder::new()
        .personal_token(personal_token)
        .build()
        .expect("failed to initialize octocrab");

    loop {
        info!("start check...");

        debug!("collect PRs to process");
        let mut pending_prs = vec![];
        let prs_stream = crab
            .pulls(&upstream_owner, &upstream_repo)
            .list()
            .state(State::Closed)
            .sort(Sort::Created)
            .direction(Direction::Descending)
            .base(MAIN_BRANCH)
            .send()
            .await
            .unwrap()
            .into_stream(&crab);
        pin!(prs_stream);
        while let Some(pr) = prs_stream.try_next().await.unwrap() {
            if pr.number <= last_mirrored_pr_id {
                debug!("no PRs to mirror");
                break;
            }

            pending_prs.push(pr);
        }
        debug!("found {} pending for mirror PRs", pending_prs.len());

        pending_prs.reverse(); // Old create first

        for pr in pending_prs {
            let pr_url = pr.html_url.unwrap();
            debug!("check PR #{} ({})", pr.number, pr_url);

            if pr.merged_at.is_none() {
                debug!("PR #{} not merged. skip", pr.number);
                continue;
            }

            info!("mirroring PR #{}", pr.number);

            // Create PR branch
            debug!("create PR mirror branch");
            let pr_title = pr.title.unwrap_or("Unknown".to_string());
            let output = Command::new("../../merge-upstream-pull-request.sh")
                .args([&pr.number.to_string(), &pr_title])
                .current_dir(REPO_DIR)
                .output()
                .expect("failed to run merge-upstream-pull-request.sh");

            // Write log
            debug!("write logs");
            File::create(format!("{}/upstream-merge-{}.log", LOGS_DIR, pr.number))
                .unwrap()
                .write_all(
                    format!(
                        "stdout:\n{}\n\nstderr:\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    )
                    .as_bytes(),
                )
                .unwrap();

            // Create Github PR
            // TODO: Skip if PR already exist
            debug!("create Github PR");
            let branch_name = format!("upstream-merge-{}", pr.number);
            let title = format!("[MIRROR] {}", &pr_title);
            let pr_body = pr.body.unwrap_or("".into());
            let body = format!("Original PR: {}\n--------------------\n{}", pr_url, pr_body);
            crab.pulls(&downstream_owner, &downstream_repo)
                .create(title, branch_name, MAIN_BRANCH)
                .body(body)
                .send()
                .await
                .expect("failed to create Github PR");

            // Write last PR id
            debug!("change last PR id");
            last_mirrored_pr_id = pr.number;
            File::create(LAST_PR_ID_FILE)
                .unwrap()
                .write_all(last_mirrored_pr_id.to_string().as_bytes())
                .unwrap();
        }

        info!("sleep...");
        std::thread::sleep(std::time::Duration::from_secs(GH_POLL_INTERVAL));
    }
}
