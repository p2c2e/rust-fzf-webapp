use axum::{
    extract::{Path, Query, State},
    response::{Html, Json},
    routing::{get, post},
    Router,
    body::Body,
    http::header,
    response::Response,
};
use chrono::{DateTime, Utc};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path as FilePath, PathBuf};
use tokio::sync::RwLock;
use walkdir::WalkDir;

#[derive(Clone)]
struct AppState {
    root_path: Arc<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Serialize)]
struct SearchResult {
    files: Vec<FileInfo>,
}

#[derive(Serialize)]
struct FileInfo {
    path: String,
    name: String,
}

async fn index() -> Html<&'static str> {
    Html(r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Fuzzy File Search</title>
            <style>
                body { 
                    font-family: Arial, sans-serif; 
                    max-width: 800px; 
                    margin: 2rem auto;
                    padding: 0 1rem;
                }
                #results { 
                    margin-top: 1rem; 
                    white-space: pre-wrap;
                }
                .search-container {
                    display: flex;
                    gap: 1rem;
                    margin-bottom: 1rem;
                }
                button {
                    padding: 0.5rem 1rem;
                }
                input {
                    flex-grow: 1;
                    padding: 0.5rem;
                }
                .file-link {
                    display: block;
                    padding: 0.5rem;
                    text-decoration: none;
                    color: #0066cc;
                }
                .file-link:hover {
                    background-color: #f0f0f0;
                }
            </style>
        </head>
        <body>
            <h1>Fuzzy File Search</h1>
            <div class="search-container">
                <input type="text" id="search" placeholder="Enter search pattern...">
                <button onclick="search()">Search</button>
                <button onclick="cancelSearch()" id="cancelBtn" style="display: none;">Cancel</button>
            </div>
            <div id="results"></div>

            <script>
                let currentController = null;

                async function search() {
                    const searchInput = document.getElementById('search');
                    const resultsDiv = document.getElementById('results');
                    const cancelBtn = document.getElementById('cancelBtn');
                    
                    if (currentController) {
                        currentController.abort();
                    }

                    currentController = new AbortController();
                    cancelBtn.style.display = 'inline';
                    resultsDiv.textContent = 'Searching...';

                    try {
                        const response = await fetch(`/search?q=${encodeURIComponent(searchInput.value)}`, {
                            signal: currentController.signal
                        });
                        const data = await response.json();
                        
                        // Clear previous results
                        resultsDiv.innerHTML = '';
                        
                        // Create links for each file
                        data.files.forEach(file => {
                            const link = document.createElement('a');
                            link.href = `/download/${encodeURIComponent(file.path)}`;
                            link.className = 'file-link';
                            link.textContent = file.name;
                            link.title = file.path; // Show full path on hover
                            resultsDiv.appendChild(link);
                        });
                        
                        if (data.files.length === 0) {
                            resultsDiv.textContent = 'No files found';
                        }
                    } catch (err) {
                        if (err.name === 'AbortError') {
                            resultsDiv.textContent = 'Search cancelled';
                        } else {
                            resultsDiv.textContent = 'Error: ' + err.message;
                        }
                    } finally {
                        currentController = null;
                        cancelBtn.style.display = 'none';
                    }
                }

                function cancelSearch() {
                    if (currentController) {
                        currentController.abort();
                    }
                }

                // Enable search on Enter key
                document.getElementById('search').addEventListener('keypress', function(e) {
                    if (e.key === 'Enter') {
                        search();
                    }
                });
            </script>
        </body>
        </html>
    "#)
}

async fn search(
    Query(query): Query<SearchQuery>,
    State(state): State<AppState>,
) -> Json<SearchResult> {
    let search_term = query.q;
    let root_path = state.root_path.as_str();

    // Create fzf command
    let mut child = tokio::process::Command::new("fzf")
        .arg("--filter")
        .arg(&search_term)
        .current_dir(root_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn fzf");

    // Use find to get only regular files (not directories)
    let find_output = tokio::process::Command::new("find")
        .arg(".")
        .arg("-type")
        .arg("f")  // Only regular files
        .current_dir(root_path)
        .output()
        .await
        .expect("Failed to execute find");

    // Write find output to fzf's stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&find_output.stdout).await.ok();
        stdin.flush().await.ok();
    }

    // Read fzf output
    let output = child.wait_with_output().await.expect("Failed to get fzf output");
    let files: Vec<FileInfo> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|s| {
            let path = PathBuf::from(s);
            // Additional check to ensure it's a file
            if path.is_file() || !path.ends_with("/") {
                Some(FileInfo {
                    path: s.to_string(),
                    name: path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(s)
                        .to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    Json(SearchResult { files })
}

async fn download_file(
    Path(file_path): Path<String>,
    State(state): State<AppState>,
) -> Response {
    let full_path = PathBuf::from(state.root_path.as_str()).join(&file_path);
    
    // Additional check to ensure we're only serving files, not directories
    if !full_path.is_file() {
        return Response::builder()
            .status(404)
            .body(Body::from("Not a file or file not found"))
            .unwrap();
    }
    
    match tokio::fs::read(&full_path).await {
        Ok(contents) => {
            let filename = full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download")
                .to_string();
            
            Response::builder()
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(contents))
                .unwrap()
        }
        Err(_) => Response::builder()
            .status(404)
            .body(Body::from("File not found"))
            .unwrap(),
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root_path = if args.len() > 1 {
        args[1].clone()
    } else {
        ".".to_string()
    };

    let state = AppState {
        root_path: Arc::new(root_path),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/search", get(search))
        .route("/download/*path", get(download_file))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("Server running on http://localhost:3000");
    
    axum::serve(
        tokio::net::TcpListener::bind(&addr)
            .await
            .unwrap(),
        app,
    )
    .await
    .unwrap();
}
