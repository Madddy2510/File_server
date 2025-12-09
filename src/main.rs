use axum::{
    Router,
    body::Body,
    extract::{State},
    http::{
        HeaderValue, StatusCode,
    },
    response::Response,
    routing::get,
};
use std::{
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    io::{self},
};
use tar::Builder;
use flate2::write::GzEncoder;
use flate2::Compression;
use local_ip_address::local_ip;

// --- Port Configuration ---
// This serves as the default port
const DEFAULT_SERVER_PORT: u16 = 8080; 
// --------------------------

#[derive(Clone)]
struct AppState {
    // Field is unused by the handler, but needed for startup, so we allow dead_code to silence the warning
    #[allow(dead_code)]
    initial_files: Vec<String>,
}

// --- ARCHIVING LOGIC ---

/// Archives a list of file paths into a tar.gz file in memory (Bytes).
fn create_tar_gz(file_paths: &[String]) -> io::Result<bytes::Bytes> {
    let mut buffer = Vec::new();
    let enc = GzEncoder::new(&mut buffer, Compression::default());
    let mut tar = Builder::new(enc);

    for file_path_str in file_paths {
        let file_path = PathBuf::from(file_path_str);
        
        if !file_path.exists() || !file_path.is_file() {
            return Err(io::Error::new(io::ErrorKind::NotFound, 
                format!("Source file not found or is a directory: {}", file_path_str)));
        }

        let file_name = file_path.file_name()
            .ok_or(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file path name"))?;

        tar.append_path_with_name(&file_path, file_name)?;
    }

    let finished_tar = tar.into_inner().unwrap();
    finished_tar.finish()?;
    
    Ok(bytes::Bytes::from(buffer))
}

// --- AXUM HANDLER (The core of the server) ---

async fn download_handler(
    State(state): State<AppState>, 
    req: axum::extract::Request,
) -> Result<Response, StatusCode> {
    
    let files_to_archive = &state.initial_files;
    let archive_filename = "archive.tar.gz";

    // 1. Generate the archive in a blocking task
    let files_for_task = files_to_archive.clone();
    
    let archive_data = match tokio::task::spawn_blocking(move || create_tar_gz(&files_for_task)).await {
        Ok(Ok(data)) => data,
        Ok(Err(e)) => {
            eprintln!("Error creating archive: {:?}", e);
            return Err(StatusCode::BAD_REQUEST);
        }
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR), 
    };
    
    let file_size = archive_data.len() as u64;

    let range_header = req.headers().get("Range");
    
    let mut res = Response::builder();
    let headers = res.headers_mut().unwrap();

    // Set general headers for all downloads
    headers.insert(axum::http::header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("application/x-tar"));
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        HeaderValue::try_from(format!("attachment; filename=\"{}\"", archive_filename)).unwrap(),
    );

    match range_header {
        Some(range_value) => {
            // --- 2. Handle Range Request (206 Partial Content) for Resume ---
            let range_str = range_value.to_str().unwrap_or("");
            
            // Parse returns Vec<HttpRange>
            let ranges: Vec<http_range::HttpRange> = http_range::HttpRange::parse(range_str, file_size)
                .map_err(|_| StatusCode::RANGE_NOT_SATISFIABLE)?;
            
            // Get the first range in the vector
            let range = ranges.into_iter().next().unwrap(); 
            
            let start = range.start;
            let length = range.length;
            let end = start + length - 1;
            let content_length = length;
            
            if start >= file_size || end >= file_size || start > end {
                eprintln!("Range check failed: {}-{}/{}", start, end, file_size);
                return Err(StatusCode::RANGE_NOT_SATISFIABLE);
            }

            // Slice the data for the requested range
            let partial_data = archive_data.slice(start as usize..=end as usize);

            // Set 206 Partial Content headers
            headers.insert(axum::http::header::CONTENT_RANGE, 
                HeaderValue::try_from(format!("bytes {}-{}/{}", start, end, file_size)).unwrap());
            headers.insert(axum::http::header::CONTENT_LENGTH, 
                HeaderValue::try_from(content_length).unwrap());

            println!("<- Responding with 206 Partial Content: bytes {}-{}/{}", start, end, file_size);
            
            Ok(res.status(StatusCode::PARTIAL_CONTENT).body(Body::from(partial_data)).unwrap())
        },
        None => {
            // --- 3. Handle Full Download Request (200 OK) ---
            headers.insert(axum::http::header::CONTENT_LENGTH, 
                HeaderValue::try_from(file_size).unwrap());
            
            println!("<- Responding with 200 OK (Full content, {} bytes)", file_size);

            Ok(res.status(StatusCode::OK).body(Body::from(archive_data)).unwrap())
        }
    }
}

// --- MAIN SETUP ---

#[tokio::main]
async fn main() {
    let mut args: Vec<String> = env::args().collect();
    
    // --- ARGUMENT PARSING LOGIC ---
    let files_start_index = 1; // args[0] is program path, files start at args[1]
    
    // 1. Check for minimum required arguments (program name + at least one file)
    if args.len() < files_start_index + 1 {
        eprintln!("Usage: cargo run -- <file_path_1> [file_path_2] [..] [optional_port]");
        eprintln!("Example: cargo run -- fileA.txt fileB.txt 8888");
        std::process::exit(1);
    }
    
    // 2. Extract arguments (excluding the program name itself)
    let mut args_without_program_name: Vec<String> = args.drain(files_start_index..).collect();
    
    let mut server_port = DEFAULT_SERVER_PORT;
    
    // 3. Check if the last argument is a valid port number
    if let Some(last_arg) = args_without_program_name.last() {
        if let Ok(port) = last_arg.parse::<u16>() {
            // It's a valid port, so use it and remove it from the list
            server_port = port;
            args_without_program_name.pop();
        }
    }

    // 4. The remaining arguments are the file paths
    let initial_files = args_without_program_name; // Now defined!

    // 5. Final validation 
    if initial_files.is_empty() {
         eprintln!("Error: You must specify at least one file path.");
         std::process::exit(1);
    }
    
    // --- END ARGUMENT PARSING LOGIC ---

    for file_path in &initial_files {
        if !Path::new(file_path).is_file() {
            eprintln!("Error: Required source file not found or is a directory: {}", file_path);
            std::process::exit(1);
        }
    }

    let local_ip_str = local_ip().map(|ip| ip.to_string()).unwrap_or_else(|e| {
        eprintln!("Warning: Could not determine local IP. Using 127.0.0.1. Error: {}", e);
        "127.0.0.1".to_string()
    });
    
    let base_url = format!("http://{}:{}", local_ip_str, server_port);
    let download_url = format!("{}/download", base_url);

    println!("--- File Archive Server Started (Axum) ---");
    println!("Files being served: {:?}", initial_files);
    println!("Server running on: {}", base_url);
    println!("----------------------------------------------------------");
    println!(" DIRECT DOWNLOAD LINK (Clickable, Port {}):", server_port);
    println!("{}", download_url);
    println!("----------------------------------------------------------");

    let app_state = AppState { initial_files };
    let app = Router::new()
        .route("/download", get(download_handler))
        .with_state(app_state);

    let addr = SocketAddr::from(([0, 0, 0, 0], server_port)); 

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            axum::serve(listener, app).await.unwrap();
        }
        Err(e) => {
            eprintln!("Error binding to address {}:{} -- Is the port already in use?", local_ip_str, server_port);
            eprintln!("Details: {}", e);
        }
    }
}