Prerequisites

You must have Rust and Cargo installed.
Build and Run

    Clone the repository:
    Bash

git clone [YOUR_REPO_URL]
cd rust-file-archive-server

Build the project (use --release for the smallest, fastest binary):
Bash

    cargo build --release

The executable will be located in target/release/rust-file-archive-server. For simplicity, you can run all commands using cargo run -- followed by the command.
🚀 Usage

The tool operates with two main commands: server and download.
1. Starting the Server

The server waits for a client to request a file list.

Syntax:
Bash

cargo run -- server [OPTIONS]

Option	Default	Description
-b, --bind-address	127.0.0.0:8080	The IP and port for the server to listen on.

Example:

To start the server and listen on all interfaces (for remote access):
Bash

# Start server on 0.0.0.0:8080
cargo run -- server -b 0.0.0.0:8080

(Note: The server will create dummy files like file1.txt if they don't exist for testing.)
2. Downloading an Archive

The client sends a list of files to the server and then handles the download, including resume logic.

Syntax:
Bash

cargo run -- download [OPTIONS] <FILES>...

Option	Default	Description
-s, --server-url	http://127.0.0.1:8080	The base URL of the running server.
-o, --output	archive.tar.gz	The name of the file to save the downloaded archive as.
<FILES>...	(Required)	A space-separated list of files the server should archive.

Example (Single Run):
Bash

# Request file1.txt and file2.txt from the server and save as my_data.tar.gz
cargo run -- download --output my_data.tar.gz --server-url http://127.0.0.1:8080 file1.txt file2.txt

Pause and Resume

To test the pause/resume functionality:

    Start the download command.

    Press Ctrl+C to stop the download mid-way.

    Run the exact same download command again.
