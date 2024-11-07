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
use std::fs;
use std::io;

#[derive(Debug, Serialize, Deserialize, Clone)]
struct IndexEntry {
    path: String,
    name: String,
    last_modified: DateTime<Utc>,
    size: u64,
}

#[derive(Clone)]
struct AppState {
    root_path: Arc<PathBuf>,
    index: Arc<RwLock<Vec<IndexEntry>>>,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Serialize)]
struct SearchResult {
    files: Vec<IndexEntry>,
}

#[derive(Serialize)]
struct IndexStatus {
    total_files: usize,
    last_updated: DateTime<Utc>,
    root_path: String,
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
            <div class="controls">
                <button onclick="createIndex()">Create/Update Index</button>
                <span id="indexStatus"></span>
            </div>
            <div class="search-container">
                <input type="text" id="search" placeholder="Enter search pattern...">
                <button onclick="search()">Search</button>
                <button onclick="cancelSearch()" id="cancelBtn" style="display: none;">Cancel</button>
            </div>
            <div id="results"></div>

            <script>
                let currentController = null;

                async function createIndex() {
                    const statusSpan = document.getElementById('indexStatus');
                    statusSpan.textContent = 'Creating index...';
                    
                    try {
                        const response = await fetch('/create-index', {
                            method: 'POST'
                        });
                        const status = await response.json();
                        statusSpan.textContent = `Indexed ${status.total_files} files`;
                    } catch (err) {
                        statusSpan.textContent = 'Error creating index: ' + err.message;
                    }
                }

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

async fn create_index(State(state): State<AppState>) -> Json<IndexStatus> {
    let mut index = state.index.write().await;
    index.clear();

    for entry in WalkDir::new(state.root_path.as_ref())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(metadata) = entry.metadata() {
            let path = entry.path().strip_prefix(state.root_path.as_ref())
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            
            index.push(IndexEntry {
                path: path.clone(),
                name: entry.file_name().to_string_lossy().to_string(),
                last_modified: metadata.modified()
                    .unwrap_or_else(|_| std::time::SystemTime::now())
                    .into(),
                size: metadata.len(),
            });
        }
    }

    let status = IndexStatus {
        total_files: index.len(),
        last_updated: Utc::now(),
        root_path: state.root_path.to_string_lossy().to_string(),
    };

    Json(status)
}

async fn search(
    Query(query): Query<SearchQuery>,
    State(state): State<AppState>,
) -> Json<SearchResult> {
    let matcher = SkimMatcherV2::default();
    let index = state.index.read().await;
    
    let mut matches: Vec<(i64, IndexEntry)> = index.iter()
        .filter_map(|entry| {
            matcher.fuzzy_match(&entry.path, &query.q)
                .map(|score| (score, entry.clone()))
        })
        .collect();

    // Sort by score descending
    matches.sort_by(|a, b| b.0.cmp(&a.0));

    Json(SearchResult {
        files: matches.into_iter().map(|(_, entry)| entry).collect()
    })
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
        root_path: Arc::new(PathBuf::from(root_path)),
        index: Arc::new(RwLock::new(Vec::new())),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/search", get(search))
        .route("/download/*path", get(download_file))
        .route("/create-index", post(create_index))
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
