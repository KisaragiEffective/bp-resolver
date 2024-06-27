#![deny(clippy::all)]
#![warn(clippy::nursery)]

use std::fs::File;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use reqwest::{Client, StatusCode};
use serde::Deserialize;

#[derive(Parser)]
enum Args {
    Resolve {
        file_path: PathBuf
    },
    Add {
    }
}

#[derive(Deserialize)]
struct PackageManifest {
    dependencies: Vec<PackageDependencyEntry>
}

#[derive(Deserialize)]
struct PackageDependencyEntry {
    source: String,
    sha512: Vec<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let token = std::env::var("PLAZA").expect("set $PLAZA");

    match args {
        Args::Resolve { file_path } => {
            let f = File::open(file_path).expect("could not open file");

            let manifest: PackageManifest = serde_json::from_reader(f).expect("valid JSON");
            let mut bags = tokio::task::JoinSet::new();
            let http_client = reqwest::ClientBuilder::new()
                .user_agent("KisaragiMarine.BpResolver/0.1.0")
                .min_tls_version(reqwest::tls::Version::TLS_1_2)
                .https_only(true)
                .build()
                .expect("failed to initialize HTTP client");
            let token = Arc::new(token);
            let http_client = Arc::new(http_client);
            println!("インストールするアセットは所有者からライセンス供与されます。");
            println!("BpResolver の開発者はサードパーティのパッケージに対して責任を負わず、ライセンスも付与しません。");

            for (i, x) in manifest.dependencies.into_iter().enumerate() {
                let http_client = Arc::clone(&http_client);
                let token = Arc::clone(&token);
                bags.spawn(async move {
                    let source = &x.source;
                    let source = iref::Uri::new(source.as_bytes());
                    let Ok(source) = source else {
                        eprintln!("sources[{i}]: malformed URI.");
                        return
                    };

                    if source.scheme().as_bytes() != b"bpdlref" {
                        eprintln!("sources[{i}]: not a BpDLRef URI.");
                        return
                    }

                    let segments = source.path().segments();
                    let segments_count = segments.count();
                    if segments_count != 1 {
                        eprintln!("sources[{i}]: URI must be `bpdlref:<downloadable ID>`");
                        return
                    }

                    let mut segments = source.path().segments();
                    let downloadable_id = segments.next().expect("unreachable");
                    let downloadable_id = downloadable_id.as_str().parse().expect("invalid downloadable id");
                    let (mime, raw) = download_or_extract_local(http_client, downloadable_id, token.as_str()).await;
                    write_packages_to_filesystem(mime, &raw).await;
                });
            }

            while let Some(join_res) = bags.join_next().await {
                join_res.expect("failed to join");
            }
        }
        Args::Add {} => {
            unimplemented!()
        }
    }
}

#[derive(Eq, PartialEq)]
enum SupportedMimeType {
    Zip,
}

async fn download_or_extract_local(http_client: Arc<Client>, downloadable_id: u32, token: &str) -> (SupportedMimeType, Vec<u8>) {
    let cache_hit = false;
    let etag: Option<&str> = None;

    println!("resolving: {downloadable_id}");
    let url = format!("https://booth.pm/downloadables/{downloadable_id}");

    let preflight = http_client.head(&url)
        .header("Cookie", token)
        .send()
        .await
        .expect("head req")
        .status();

    if preflight == StatusCode::NOT_FOUND {
        panic!("no such downloadable: {downloadable_id}");
    } else if preflight == StatusCode::FORBIDDEN {
        panic!("no such downloadable: forbidden: {downloadable_id}");
    }

    println!("preflight: {preflight}");

    let req = http_client
        .get(url)
        .header("Cookie", token);

    let req = if let Some(etag) = etag {
        req.header("If-None-Match", etag)
    } else {
        req
    };

    let res = req.send().await.expect("request sent");
    if etag.is_some() && res.status() == StatusCode::NO_CONTENT {
        // TODO: re-use local cache
    } else {
        let bytes = res.content_length().expect("must be present");
        println!("len: {bytes}");
    }

    let content_type = res.headers().get("content-type").expect("response must have content-type header").as_bytes();
    let matcher = match content_type {
        b"application/zip" => SupportedMimeType::Zip,
        other => panic!("content-type: {} is not supported yet", String::from_utf8_lossy(other)),
    };

    // TODO: progress bar
    let body = res.bytes().await.expect("whole response");

    (matcher, body.to_vec())
}

async fn write_packages_to_filesystem(mime: SupportedMimeType, raw_content: &[u8]) -> PathBuf {
    if mime == SupportedMimeType::Zip {
        let tempdir = tempfile::tempdir().expect("a");
        let path = tempdir.path();
        zip::read::ZipArchive::new(Cursor::new(raw_content)).expect("valid ZIP archive")
            .extract(path)
            .unwrap();

        println!("extracted to {}", path.display());

        // do not delete!
        tempdir.into_path()
    } else {
        unreachable!()
    }
}
