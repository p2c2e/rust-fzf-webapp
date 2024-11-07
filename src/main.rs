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
use std::net::SocketAddr;
use std::path::PathBuf;
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

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    recent_paths: Vec<String>,
}

#[derive(Clone)]
struct AppState {
    root_path: Arc<PathBuf>,
    index: Arc<RwLock<Vec<IndexEntry>>>,
    config: Arc<RwLock<Config>>,
}

impl Config {
    fn load() -> io::Result<Self> {
        let config_path = get_config_path()?;
        if config_path.exists() {
            let contents = fs::read_to_string(config_path)?;
            Ok(serde_json::from_str(&contents).unwrap_or(Config { recent_paths: vec![] }))
        } else {
            Ok(Config { recent_paths: vec![] })
        }
    }

    fn save(&self) -> io::Result<()> {
        let config_path = get_config_path()?;
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = serde_json::to_string_pretty(self)?;
        fs::write(config_path, contents)
    }

    fn add_path(&mut self, path: String) {
        if !self.recent_paths.contains(&path) {
            self.recent_paths.insert(0, path);
            if self.recent_paths.len() > 5 {
                self.recent_paths.pop();
            }
        }
    }
}

fn get_config_path() -> io::Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("", "", "rsconfig")
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Could not determine config directory"))?;
    Ok(proj_dirs.config_dir().join("config.json"))
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
            <div id="currentPath" style="background: #f0f0f0; padding: 10px; margin: 10px 0; border-radius: 4px;">
                Current Path: <span id="pathDisplay"></span>
            </div>
            <div class="controls">
                <button onclick="openDirectoryBrowser()">Browse Directories</button>
                <select id="pathSelect" onchange="changePath(this.value)">
                    <option value="">Select a recent path...</option>
                </select>
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

                // Load recent paths on page load
                window.addEventListener('load', async () => {
                    const response = await fetch('/recent-paths');
                    const paths = await response.json();
                    const select = document.getElementById('pathSelect');
                    
                    paths.forEach(path => {
                        const option = document.createElement('option');
                        option.value = path;
                        option.textContent = path;
                        select.appendChild(option);
                    });
                });

                async function changePath(path) {
                    if (!path) return;
                    
                    const statusSpan = document.getElementById('indexStatus');
                    statusSpan.textContent = 'Loading index for ' + path + '...';
                    
                    try {
                        const response = await fetch('/change-path', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json',
                            },
                            body: JSON.stringify({ path }),
                        });
                        const result = await response.json();
                        statusSpan.textContent = `Loaded index with ${result.total_files} files`;
                    } catch (err) {
                        statusSpan.textContent = 'Error changing path: ' + err.message;
                    }
                }

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
                async function openDirectoryBrowser() {
                    const currentPath = document.getElementById('pathDisplay').textContent || '/';
                    try {
                        const response = await fetch(`/list-directories/${encodeURIComponent(currentPath)}`);
                        const dirs = await response.json();
                        
                        const modal = document.createElement('div');
                        modal.style.cssText = `
                            position: fixed;
                            top: 50%;
                            left: 50%;
                            transform: translate(-50%, -50%);
                            background: white;
                            padding: 20px;
                            border-radius: 8px;
                            box-shadow: 0 2px 10px rgba(0,0,0,0.1);
                            max-width: 80%;
                            max-height: 80vh;
                            overflow-y: auto;
                            z-index: 1000;
                        `;
                        
                        const parentLink = document.createElement('a');
                        parentLink.href = '#';
                        parentLink.textContent = '📁 ..';
                        parentLink.onclick = (e) => {
                            e.preventDefault();
                            const parentPath = currentPath.split('/').slice(0, -1).join('/') || '/';
                            changePath(parentPath);
                            modal.remove();
                        };
                        modal.appendChild(parentLink);
                        modal.appendChild(document.createElement('br'));
                        
                        dirs.forEach(dir => {
                            const link = document.createElement('a');
                            link.href = '#';
                            link.textContent = `📁 ${dir.split('/').pop()}`;
                            link.style.display = 'block';
                            link.style.padding = '5px 0';
                            link.onclick = (e) => {
                                e.preventDefault();
                                changePath(dir);
                                modal.remove();
                            };
                            modal.appendChild(link);
                        });
                        
                        document.body.appendChild(modal);
                    } catch (err) {
                        console.error('Error listing directories:', err);
                    }
                }

                // Update path display when path changes
                function updatePathDisplay(path) {
                    document.getElementById('pathDisplay').textContent = path;
                }

                // Modify existing changePath function
                async function changePath(path) {
                    if (!path) return;
                    
                    const statusSpan = document.getElementById('indexStatus');
                    statusSpan.textContent = 'Changed to: ' + path;
                    updatePathDisplay(path);
                    
                    try {
                        const response = await fetch('/change-path', {
                            method: 'POST',
                            headers: {
                                'Content-Type': 'application/json',
                            },
                            body: JSON.stringify({ path }),
                        });
                        const result = await response.json();
                        statusSpan.textContent = `Changed to ${path} (${result.total_files} files indexed)`;
                    } catch (err) {
                        statusSpan.textContent = 'Error changing path: ' + err.message;
                    }
                }

                // Initialize path display on load
                window.addEventListener('load', () => {
                    const initialPath = new URLSearchParams(window.location.search).get('path') || '/';
                    updatePathDisplay(initialPath);
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
    let full_path = state.root_path.as_ref().join(&file_path);
    
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

#[derive(Deserialize)]
struct ChangePathRequest {
    path: String,
}

async fn get_recent_paths(State(state): State<AppState>) -> Json<Vec<String>> {
    let config = state.config.read().await;
    Json(config.recent_paths.clone())
}

async fn change_path(
    State(state): State<AppState>,
    Json(req): Json<ChangePathRequest>,
) -> Json<IndexStatus> {
    // Update config
    let new_path = PathBuf::from(&req.path);
    // Create new state with updated path
    let state = AppState {
        root_path: Arc::new(new_path),
        index: state.index,
        config: state.config,
    };
    {
        let mut config = state.config.write().await;
        config.add_path(req.path);
        let _ = config.save();
    }
    
    // Create new index
    create_index(State(state.clone())).await
}

async fn list_directories(Path(current_path): Path<String>) -> Json<Vec<String>> {
    let path = PathBuf::from(current_path);
    let mut dirs = Vec::new();
    
    if let Ok(entries) = fs::read_dir(&path) {
        for entry in entries.filter_map(|e| e.ok()) {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    if let Some(path_str) = entry.path().to_str().map(String::from) {
                        dirs.push(path_str);
                    }
                }
            }
        }
    }
    
    dirs.sort();
    Json(dirs)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tokio::runtime::Runtime::new()?.block_on(async {
    let root_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    let config = Config::load().unwrap_or_else(|_| Config { recent_paths: vec![] });
    
    let state = AppState {
        root_path: Arc::new(PathBuf::from(root_path.clone())),
        index: Arc::new(RwLock::new(Vec::new())),
        config: Arc::new(RwLock::new(config)),
    };
    
    // Add initial path to config
    {
        let mut config = state.config.write().await;
        config.add_path(root_path);
        let _ = config.save();
    }

    let app = Router::new()
        .route("/", get(index))
        .route("/search", get(search))
        .route("/download/*path", get(download_file))
        .route("/create-index", post(create_index))
        .route("/recent-paths", get(get_recent_paths))
        .route("/change-path", post(change_path))
        .route("/list-directories/:path", get(list_directories))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    println!("Server running on http://localhost:3000");
    
    axum::serve(
        tokio::net::TcpListener::bind(&addr).await?,
        app,
    )
    .await?;

        Ok(())
    })
}
