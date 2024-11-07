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

impl IndexEntry {
    fn save_index(entries: &[IndexEntry], root_path: &PathBuf) -> io::Result<()> {
        let index_dir = get_index_dir()?;
        fs::create_dir_all(&index_dir)?;
        
        // Create a unique filename based on the root path
        let path_hash = format!("{:x}", md5::compute(root_path.to_string_lossy().as_bytes()));
        let index_path = index_dir.join(format!("index_{}.json", path_hash));
        
        let contents = serde_json::to_string_pretty(entries)?;
        fs::write(index_path, contents)
    }

    fn load_index(root_path: &PathBuf) -> io::Result<Vec<IndexEntry>> {
        let index_dir = get_index_dir()?;
        let path_hash = format!("{:x}", md5::compute(root_path.to_string_lossy().as_bytes()));
        let index_path = index_dir.join(format!("index_{}.json", path_hash));

        if index_path.exists() {
            let contents = fs::read_to_string(index_path)?;
            Ok(serde_json::from_str(&contents)?)
        } else {
            Ok(Vec::new())
        }
    }
}

fn get_index_dir() -> io::Result<PathBuf> {
    let proj_dirs = directories::ProjectDirs::from("", "", "rsconfig")
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Could not determine index directory"))?;
    let index_dir = proj_dirs.cache_dir().join("indices");
    println!("Index directory: {}", index_dir.display());
    Ok(index_dir)
}

#[derive(Debug, Serialize, Deserialize)]
struct PathConfig {
    path: String,
    last_indexed: Option<DateTime<Utc>>,
    total_files: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    recent_paths: Vec<PathConfig>,
}

use std::collections::HashMap;

#[derive(Clone)]
struct AppState {
    root_path: Arc<PathBuf>,
    indices: Arc<RwLock<HashMap<String, Vec<IndexEntry>>>>,
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

    fn add_path(&mut self, path: String, total_files: usize) {
        if let Some(existing) = self.recent_paths.iter_mut()
            .find(|p| p.path == path) {
            existing.last_indexed = Some(Utc::now());
            existing.total_files = total_files;
        } else {
            self.recent_paths.insert(0, PathConfig {
                path,
                last_indexed: Some(Utc::now()),
                total_files,
            });
            if self.recent_paths.len() > 5 {
                self.recent_paths.pop();
            }
        }
    }

    fn get_paths(&self) -> Vec<String> {
        self.recent_paths.iter().map(|p| p.path.clone()).collect()
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
                .controls {
                    display: grid;
                    grid-template-columns: 1fr 1fr;
                    gap: 0.5rem;
                    margin-bottom: 1rem;
                }
                .search-container {
                    display: grid;
                    grid-template-columns: 1fr auto;
                    gap: 0.5rem;
                    margin-bottom: 1rem;
                }
                #results { 
                    height: 400px;
                    overflow-y: auto;
                    border: 1px solid #ddd;
                    padding: 0.5rem;
                    margin-top: 0.5rem;
                }
                button {
                    padding: 0.5rem 1rem;
                    width: 100%;
                }
                input, select {
                    padding: 0.5rem;
                    width: 100%;
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
                <div id="indexStatus" style="font-size: 0.9em; color: #666;"></div>
            </div>
            <div class="controls">
                <select id="pathSelect" onchange="changePath(this.value)">
                    <option value="">Select recent path...</option>
                </select>
                <button onclick="createIndex()">Create/Update Index</button>
                <button onclick="purgeIndices()" style="background-color: #ff4444; color: white;">Purge All Indices</button>
                <button onclick="openDirectoryBrowser()">Browse Directories</button>
            </div>
            <div class="search-container">
                <input type="text" id="search" placeholder="Search query...">
                <button onclick="search()">Search</button>
            </div>
            <div id="results">
                <div class="results-header">Search results: (only 25 rows visible)</div>
            </div>
            <button onclick="cancelSearch()" id="cancelBtn" style="display: none;">Cancel</button>

            <script>
                let currentController = null;

                // Load recent paths on page load
                window.addEventListener('load', async () => {
                    const response = await fetch('/recent-paths');
                    const paths = await response.json();
                    const select = document.getElementById('pathSelect');
                    
                    paths.forEach(pathConfig => {
                        const option = document.createElement('option');
                        option.value = pathConfig.path;
                        const lastIndexed = pathConfig.last_indexed ? 
                            new Date(pathConfig.last_indexed).toLocaleString() : 'Never';
                        option.textContent = `${pathConfig.path} (${pathConfig.total_files} files, indexed: ${lastIndexed})`;
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

                async function purgeIndices() {
                    if (!confirm('Are you sure you want to delete all saved indices?')) {
                        return;
                    }
                    
                    const statusSpan = document.getElementById('indexStatus');
                    statusSpan.textContent = 'Purging all indices...';
                    
                    try {
                        const response = await fetch('/purge-indices', {
                            method: 'POST'
                        });
                        const result = await response.json();
                        statusSpan.textContent = result;
                    } catch (err) {
                        statusSpan.textContent = 'Error purging indices: ' + err.message;
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
                        // Only show first 25 results
                        data.files.slice(0, 25).forEach(file => {
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
                    
                    // Create modal overlay
                    const overlay = document.createElement('div');
                    overlay.style.cssText = `
                        position: fixed;
                        top: 0;
                        left: 0;
                        width: 100%;
                        height: 100%;
                        background: rgba(0,0,0,0.5);
                        z-index: 999;
                    `;
                    
                    async function loadDirectory(path) {
                        try {
                            const response = await fetch(`/list-directories/${encodeURIComponent(path)}`);
                            const dirs = await response.json();
                            
                            // Update current path display
                            pathDisplay.textContent = path;
                            
                            // Clear and rebuild directory list
                            dirList.innerHTML = '';
                            
                            dirs.forEach(dir => {
                                const link = document.createElement('a');
                                link.href = '#';
                                const isParent = dir === dirs[0] && path !== '/';
                                link.textContent = isParent ? '📁 ..' : `📁 ${dir.split('/').pop()}`;
                                link.style.cssText = `
                                    display: block;
                                    padding: 8px;
                                    text-decoration: none;
                                    color: #333;
                                    border-bottom: 1px solid #eee;
                                `;
                                link.onmouseover = () => { link.style.backgroundColor = '#f0f0f0'; };
                                link.onmouseout = () => { link.style.backgroundColor = 'transparent'; };
                                link.onclick = async (e) => {
                                    e.preventDefault();
                                    await loadDirectory(dir);
                                };
                                dirList.appendChild(link);
                            });
                        } catch (err) {
                            console.error('Error listing directories:', err);
                        }
                    }
                    
                    // Create modal content
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
                        width: 80%;
                        max-width: 600px;
                        max-height: 80vh;
                        display: flex;
                        flex-direction: column;
                        z-index: 1000;
                    `;
                    
                    // Create header with current path display
                    const header = document.createElement('div');
                    header.style.marginBottom = '10px';
                    const pathDisplay = document.createElement('div');
                    pathDisplay.style.cssText = `
                        font-weight: bold;
                        padding: 8px;
                        background: #f5f5f5;
                        border-radius: 4px;
                        margin-bottom: 10px;
                    `;
                    header.appendChild(pathDisplay);
                    
                    // Create scrollable directory list
                    const dirList = document.createElement('div');
                    dirList.style.cssText = `
                        overflow-y: auto;
                        flex-grow: 1;
                        border: 1px solid #eee;
                        border-radius: 4px;
                    `;
                    
                    // Create footer with buttons
                    const footer = document.createElement('div');
                    footer.style.cssText = `
                        margin-top: 15px;
                        display: flex;
                        justify-content: flex-end;
                        gap: 10px;
                    `;
                    
                    const selectButton = document.createElement('button');
                    selectButton.textContent = 'Select Directory';
                    selectButton.onclick = () => {
                        const selectedPath = pathDisplay.textContent;
                        changePath(selectedPath);
                        document.body.removeChild(overlay);
                    };
                    
                    const cancelButton = document.createElement('button');
                    cancelButton.textContent = 'Cancel';
                    cancelButton.onclick = () => {
                        document.body.removeChild(overlay);
                    };
                    
                    footer.appendChild(cancelButton);
                    footer.appendChild(selectButton);
                    
                    // Assemble modal
                    modal.appendChild(header);
                    modal.appendChild(dirList);
                    modal.appendChild(footer);
                    
                    // Add modal to overlay
                    overlay.appendChild(modal);
                    document.body.appendChild(overlay);
                    
                    // Load initial directory
                    await loadDirectory(currentPath);
                }

                // Update path display when path changes
                function updatePathDisplay(path) {
                    document.getElementById('pathDisplay').textContent = path;
                }

                // Modify existing changePath function
                async function changePath(path) {
                    if (!path) return;
                    
                    const statusSpan = document.getElementById('indexStatus');
                    if (statusSpan) {
                        statusSpan.textContent = 'Changed to: ' + path;
                    }
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
                        if (statusSpan) {
                            statusSpan.textContent = `Changed to ${path} (${result.total_files} files indexed)`;
                        }
                    } catch (err) {
                        if (statusSpan) {
                            statusSpan.textContent = 'Error changing path: ' + err.message;
                        }
                        console.error('Error changing path:', err);
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
    let root_path = state.root_path.clone();
    println!("Creating index for root path: {}", root_path.display());
    
    let mut new_index = Vec::new();
    for entry in WalkDir::new(root_path.as_ref())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        if let Ok(metadata) = entry.metadata() {
            let full_path = entry.path();
            let path = entry.path().strip_prefix(state.root_path.as_ref())
                .unwrap_or(entry.path())
                .to_string_lossy()
                .to_string();
            
            println!("Indexing file: {} (relative path: {})", full_path.display(), path);
            
            new_index.push(IndexEntry {
                path: path.clone(),
                name: entry.file_name().to_string_lossy().to_string(),
                last_modified: metadata.modified()
                    .unwrap_or_else(|_| std::time::SystemTime::now())
                    .into(),
                size: metadata.len(),
            });
        }
    }

    // Update the indices map with the new index
    {
        let mut indices = state.indices.write().await;
        indices.insert(root_path.to_string_lossy().to_string(), new_index.clone());
    }

    let status = IndexStatus {
        total_files: new_index.len(),
        last_updated: Utc::now(),
        root_path: root_path.to_string_lossy().to_string(),
    };

    // Save the index to disk
    if let Err(e) = IndexEntry::save_index(&new_index, &root_path) {
        println!("Error saving index: {}", e);
    } else {
        println!("Index saved successfully");
    }

    Json(status)
}

async fn search(
    Query(query): Query<SearchQuery>,
    State(state): State<AppState>,
) -> Json<SearchResult> {
    let matcher = SkimMatcherV2::default();
    let indices = state.indices.read().await;
    
    // Get the current path's index
    let current_path = state.root_path.to_string_lossy().to_string();
    let empty_vec = Vec::new();
    let index = indices.get(&current_path).unwrap_or(&empty_vec);
    
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
    println!("Download request for file: {}", file_path);
    println!("Root path is: {}", state.root_path.display());

    // Clean the file path and convert to PathBuf
    let file_path = PathBuf::from(file_path.trim_start_matches('/'));
    if file_path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        println!("Rejected due to parent directory traversal attempt");
        return Response::builder()
            .status(403)
            .body(Body::from("Invalid path"))
            .unwrap();
    }

    let full_path = state.root_path.join(&file_path);
    println!("Full path constructed: {}", full_path.display());
    
    // Additional check to ensure we're only serving files within root_path
    if !full_path.starts_with(&*state.root_path) {
        println!("Rejected: Path {} is outside root path {}", full_path.display(), state.root_path.display());
        return Response::builder()
            .status(404)
            .body(Body::from("File path outside root directory"))
            .unwrap();
    }

    if !full_path.is_file() {
        println!("Rejected: Path {} is not a file", full_path.display());
        return Response::builder()
            .status(404)
            .body(Body::from("Not a file"))
            .unwrap();
    }
    
    match tokio::fs::read(&full_path).await {
        Ok(contents) => {
            let filename = full_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("download")
                .to_string();
            
            println!("Successfully read file: {} ({} bytes)", filename, contents.len());
            
            Response::builder()
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", filename),
                )
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(contents))
                .unwrap()
        }
        Err(e) => {
            println!("Error reading file {}: {}", full_path.display(), e);
            Response::builder()
                .status(404)
                .body(Body::from(format!("Error reading file: {}", e)))
                .unwrap()
        }
    }
}

#[derive(Deserialize)]
struct ChangePathRequest {
    path: String,
}

async fn get_recent_paths(State(state): State<AppState>) -> Json<Vec<PathConfig>> {
    let config = state.config.read().await;
    Json(config.recent_paths.clone())
}

async fn change_path(
    State(state): State<AppState>,
    Json(req): Json<ChangePathRequest>,
) -> Json<IndexStatus> {
    println!("Changing path to: {}", req.path);
    
    // Update the root path in the existing state
    let new_path = PathBuf::from(&req.path);
    *Arc::make_mut(&mut Arc::clone(&state.root_path)) = new_path.clone();
    println!("Updated root path to: {}", state.root_path.display());
    
    // Try to load existing index for the new path
    let loaded_index = IndexEntry::load_index(&new_path).unwrap_or_else(|e| {
        println!("Could not load existing index for {}: {}", new_path.display(), e);
        Vec::new()
    });

    // Update the indices map with the loaded index
    {
        let mut indices = state.indices.write().await;
        indices.insert(new_path.to_string_lossy().to_string(), loaded_index.clone());
        println!("Loaded existing index with {} entries", loaded_index.len());
    }
    
    // Update config with new path
    {
        let mut config = state.config.write().await;
        config.add_path(req.path.clone(), loaded_index.len());
        let _ = config.save();
        println!("Updated config with new path");
    }

    // Return current index status
    Json(IndexStatus {
        total_files: state.indices.read().await.get(&req.path).map(|idx| idx.len()).unwrap_or(0),
        last_updated: Utc::now(),
        root_path: state.root_path.to_string_lossy().to_string(),
    })
}

async fn purge_indices() -> Json<String> {
    if let Ok(index_dir) = get_index_dir() {
        if let Err(e) = fs::remove_dir_all(&index_dir) {
            return Json(format!("Error purging indices: {}", e));
        }
    }
    Json("All indices purged successfully".to_string())
}

async fn list_directories(Path(current_path): Path<String>) -> Json<Vec<String>> {
    let path = PathBuf::from(current_path);
    let mut dirs = Vec::new();
    
    // Add parent directory if not at root
    if path != PathBuf::from("/") {
        if let Some(parent) = path.parent() {
            if let Some(parent_str) = parent.to_str() {
                dirs.push(parent_str.to_string());
            }
        }
    }
    
    // List current directory contents
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
    // Start with current directory as default
    let root_path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    let config = Config::load().unwrap_or_else(|_| Config { recent_paths: vec![] });
    
    // Try to load existing index
    let mut initial_indices = HashMap::new();
    let initial_index = IndexEntry::load_index(&PathBuf::from(&root_path))
        .unwrap_or_else(|e| {
            println!("Could not load existing index: {}", e);
            Vec::new()
        });
    initial_indices.insert(root_path.clone(), initial_index.clone());
    
    // Try to load existing index
    let mut initial_indices = HashMap::new();
    let initial_index = IndexEntry::load_index(&PathBuf::from(&root_path))
        .unwrap_or_else(|e| {
            println!("Could not load existing index: {}", e);
            Vec::new()
        });
    initial_indices.insert(root_path.clone(), initial_index);

    let state = AppState {
        root_path: Arc::new(PathBuf::from(&root_path)),
        indices: Arc::new(RwLock::new(initial_indices)),
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
        .route("/purge-indices", post(purge_indices))
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
