# Fuzzy Search Webapp

A fast fuzzy file search web application built with Rust and Axum.

## Features

- Fast fuzzy file search
- Web-based interface
- Recent paths history
- Real-time search results

## Prerequisites

- Rust 1.70 or higher
- Cargo (Rust's package manager)

## Building

To build the release version of the application:

```bash
cargo build --release
```

The executable will be created in `target/release/fuzzy-search-webapp`

## Development

To run the development version:

```bash
cargo run
```

## Installation

After building, you can copy the executable to a location in your PATH:

### macOS/Linux
```bash
sudo cp target/release/fuzzy-search-webapp /usr/local/bin/
```

## Usage

1. Start the application:
```bash
fuzzy-search-webapp
```

2. Open your web browser and navigate to `http://localhost:3000`

3. Use the interface to:
   - Select directories to search
   - Perform fuzzy searches
   - View recent search paths

## Configuration

The application stores its configuration in:
- macOS: `~/Library/Application Support/rsconfig/config.json`
- Linux: `~/.config/rsconfig/config.json`
- Windows: `%APPDATA%\rsconfig\config.json`
