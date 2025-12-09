use clap::Parser;
use tokio::io::{AsyncWriteExt, self as tokio_io};
use actix_web::{web, App, HttpServer, HttpRequest, HttpResponse, Responder};
use reqwest::header::{RANGE, HeaderValue};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tar::Builder;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::StreamExt;

// --- CLI STRUCTURE ---

/// A File Archiving and Serving CLI Tool
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Starts the file server to listen for download requests
    Server {
        /// The IP address and port to listen on (e.g., 127.0.0.1:8080)
        #[clap(short, long, default_value = "127.0.0.1:8080")]
        bind_address: String,
    },
    /// Requests and downloads a tar.gz archive from a server
    Download {
        /// The files to be archived and downloaded (path relative to server)
        #[clap(required = true)]
        files: Vec<String>,

        /// The server URL (e.g., http://127.0.0.1:8080)
        #[clap(short, long, default_value = "http://127.0.0.1:8080")]
        server_url: String,

        /// The name of the output archive file
        #[clap(short, long, default_value = "archive.tar.gz")]
        output: String,
    },
}

// --- ARCHIVING LOGIC ---

/// Archives a list of file paths into a tar.gz file at the given output path.
fn create_tar_gz(file_paths: &[String], output_path: &Path) -> io::Result<()> {
    println!("  -> Archiving files into: {}", output_path.display());

    let file = std::fs::File::create(output_path)?;
    let enc = GzEncoder::new(file, Compression::default());
    let mut tar = Builder::new(enc);

    for file_path_str in file_paths {
        let file_path = PathBuf::from(file_path_str);
        if !file_path.exists() {
            eprintln!("  ⚠️ Warning: File not found: {}", file_path_str);
            continue;
        }

        let file_name = file_path.file_name()
            .ok_or(io::Error::new(io::ErrorKind::InvalidInput, "Invalid file path"))?;

        // Append the file to the archive
        tar.append_path_with_name(&file_path, file_name)?;
        println!("    - Added: {}", file_path_str);
    }

    tar.finish()?;
    println!("  -> Archiving complete.");
    Ok(())
}

// --- SERVER HANDLER ---

/// Handles the archive download request, including Range headers for pause/resume.
async fn download_archive(req: HttpRequest, file_list: web::Data<Vec<String>>) -> impl Responder {
    let temp_archive_path = PathBuf::from("temp_archive_data.tar.gz");

    // 1. Create the archive on the fly (or check cache/pre-generation)
    if let Err(e) = create_tar_gz(&file_list, &temp_archive_path) {
        eprintln!("Error creating archive: {}", e);
        return HttpResponse::InternalServerError().body(format!("Failed to create archive: {}", e));
    }

    let mut file = match std::fs::File::open(&temp_archive_path) {
        Ok(f) => f,
        Err(_) => return HttpResponse::NotFound().body("Archive file not found on server."),
    };

    let file_size = match file.metadata() {
        Ok(meta) => meta.len(),
        Err(_) => return HttpResponse::InternalServerError().body("Could not get archive size."),
    };

    let range_header = req.headers().get("Range").and_then(|h| h.to_str().ok());

    match range_header {
        Some(range_str) => {
            // Handle Range Request (206 Partial Content)
            let range_str = range_str.trim_start_matches("bytes=");
            let parts: Vec<&str> = range_str.split('-').collect();
            
            let start = parts.get(0).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            
            // If the range is just "bytes=start-", assume until the end
            let end = parts.get(1)
                .filter(|s| !s.is_empty())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(file_size.saturating_sub(1));
            
            if start >= file_size || start > end {
                return HttpResponse::RangeNotSatisfiable()
                    .insert_header(("Content-Range", format!("bytes */{}", file_size)))
                    .body("Invalid range request");
            }
            
            let content_length = end.saturating_sub(start).saturating_add(1);

            // Seek and read the chunk
            if file.seek(SeekFrom::Start(start)).is_err() {
                return HttpResponse::InternalServerError().body("Seek error.");
            }
            let mut buffer = vec![0; content_length as usize];
            if file.read_exact(&mut buffer).is_err() {
                 return HttpResponse::InternalServerError().body("Read error.");
            }

            println!("  <- Responding with 206 Partial Content: bytes {}-{}/{}", start, end, file_size);

            HttpResponse::PartialContent() // Status 206
                .content_type("application/x-tar")
                .insert_header(("Content-Range", format!("bytes {}-{}/{}", start, end, file_size)))
                .insert_header(("Content-Length", content_length))
                .body(buffer)
        },
        None => {
            // Handle Full Download Request (200 OK)
            println!("  <- Responding with 200 OK (Full content)");

            let content = match std::fs::read(&temp_archive_path) {
                Ok(c) => c,
                Err(_) => return HttpResponse::InternalServerError().body("Failed to read archive file."),
            };

            HttpResponse::Ok() // Status 200
                .content_type("application/x-tar")
                .insert_header(("Content-Length", file_size))
                .body(content)
        }
    }
}

// --- SERVER STARTUP ---

async fn start_server(bind_address: &str, files: Vec<String>) -> io::Result<()> {
    let file_data = web::Data::new(files);
    
    // Safety check: Create a dummy file for the archive logic to find, if not present
    std::fs::write("temp_archive_data.tar.gz", "placeholder")?;

    println!("🌍 Server listening on http://{}", bind_address);
    println!("Serving files: {:?}", file_data.get_ref());

    HttpServer::new(move || {
        App::new()
            .app_data(file_data.clone())
            .route("/download", web::get().to(download_archive))
    })
    .bind(bind_address)?
    .run()
    .await
}

// --- CLIENT LOGIC ---

/// Initiates the download, checking for an existing file to resume.
async fn start_download(files: &[String], server_url: &str, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // 1. Construct the files query parameter for the server to know what to archive
    let file_params: Vec<(&str, &str)> = files.iter().map(|f| ("files", f.as_str())).collect();
    let url = reqwest::Url::parse_with_params(&format!("{}/download", server_url), &file_params)
        .expect("Failed to create request URL.");

    let mut start_byte: u64 = 0;

    // 2. Check for existing file to determine resume point
    if let Ok(metadata) = tokio::fs::metadata(output).await {
        start_byte = metadata.len();
        println!("  ➡️ Found partial file. Resuming download from byte: {}", start_byte);
    } else {
        println!("  ➡️ Starting fresh download.");
    }

    // 3. Build the request with the Range header if resuming
    let mut request = client.get(url);
    if start_byte > 0 {
        let range_value = format!("bytes={}-", start_byte);
        request = request.header(RANGE, HeaderValue::from_str(&range_value)?);
    }

    let response = request.send().await?.error_for_status()?;

    let status = response.status();
    println!("  <- Server Status: {}", status);

    if status.is_success() && status.as_u16() != 206 {
        // Full download (200 OK) - truncate file if it existed
        start_byte = 0;
    } else if status.as_u16() == 206 && start_byte == 0 {
        println!("  ⚠️ Warning: Got Partial Content (206) response without an initial offset.");
    }


    // 4. Open the output file
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(start_byte > 0) // Append if resuming, overwrite if 200 OK
        .truncate(start_byte == 0 && status.as_u16() != 206)
        .open(output)
        .await?;

    // 5. Stream the response data
    let mut stream = response.bytes_stream();
    let mut downloaded_bytes = 0;

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result?;
        file.write_all(&chunk).await?;
        downloaded_bytes += chunk.len();
        
        // Simple progress indicator
        if downloaded_bytes % (1024 * 50) == 0 {
            print!("\r  📥 Downloaded: {} KB", (start_byte + downloaded_bytes as u64) / 1024);
            tokio_io::stdout().flush().await?;
        }
    }
    
    // Final flush and message
    tokio_io::stdout().flush().await?;
    println!("\n✅ Download complete. Total bytes written: {}", start_byte + downloaded_bytes as u64);

    Ok(())
}


// --- MAIN EXECUTION ---

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Server { bind_address } => {
            // NOTE: In a real app, the server would need to know WHICH files to serve
            // This example hardcodes a list for demonstration, assuming the files exist
            // in the server's working directory.
            let dummy_files = vec![
                "file1.txt".to_string(), 
                "file2.txt".to_string(),
                "file3.txt".to_string(),
            ];
            
            // To make the example runnable, create the dummy files first
            for (i, name) in dummy_files.iter().enumerate() {
                if tokio::fs::metadata(name).await.is_err() {
                    tokio::fs::write(name, format!("This is the content of {} - size: {}\n", name, i)).await?;
                }
            }

            println!("🚀 Starting File Archiving Server...");
            start_server(&bind_address, dummy_files).await?;
        }
        Commands::Download { files, server_url, output } => {
            println!("⬇️  Initiating Download Command...");
            start_download(&files, &server_url, &output).await?;
        }
    }

    // Clean up the temporary archive file after server shutdown (if server logic ran)
    // NOTE: This cleanup is incomplete in a real server lifecycle but included for good measure.
    let _ = tokio::fs::remove_file("temp_archive_data.tar.gz").await;
    
    Ok(())
}