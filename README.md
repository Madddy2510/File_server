
1.Prerequisites
You must have **Rust** and **Cargo** installed 


2.Build```bash cargo build```  --release

3.Usage 
    Run the server by providing a list of files to archive. 
        The port is optional.
             A. Use Default Port (8080):Bashcargo run -- ./file1.txt ./log.txt
             B. Specify Custom Port (e.g., 5000):Bashcargo run -- ./file1.txt ./log.txt 5000.
    The server will print a direct, clickable download link (e.g., http://192.168.1.32:5000/download).

                
3.Testing 
    Resume To test pause/resume, use wget in a separate terminal:
        Start download and immediately hit Ctrl+C: wget http://[IP]:[PORT]/download
        Resume download: wget -c http://[IP]:[PORT]/download
