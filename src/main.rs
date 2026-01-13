use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};

type Tx = mpsc::Sender<String>;
type Agents = Arc<RwLock<HashMap<String, Tx>>>;

// type MessageQueue = Arc<RwLock<HashMap<String, Vec<String>>>>;

const IP: &str = "0.0.0.0";
const PORT: u16 = 3939;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = format!("{}:{}", IP, PORT);
    println!("server listening on {}", addr);
    let listener = TcpListener::bind(addr).await?;

    let agents: Agents = Arc::new(RwLock::new(HashMap::new()));

    loop {
        let (stream, peer) = listener.accept().await?;
        let agents = Arc::clone(&agents);

        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, agents).await {
                eprintln!("[{}] connection error: {}", peer, e);
            }
        });
    }
}

async fn handle_connection(stream: TcpStream, agents: Agents) -> std::io::Result<()> {
    let peer = stream.peer_addr()?;
    let (reader, mut writer) = stream.into_split();

    // Канал на запись этому клиенту
    let (tx, mut rx) = mpsc::channel::<String>(128);

    // Задача, которая пишет в сокет всё, что приходит по rx
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            // Каждое сообщение — отдельной строкой
            if writer.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
            if writer.write_all(b"\n").await.is_err() {
                break;
            }
        }
    });

    // Инициализируем reader для основного цикла
    let mut lines = BufReader::new(reader).lines();

    tx.send("Enter your name (#username).".to_string())
        .await
        .ok();

    let mut name: Option<String> = None;

    // Основной цикл чтения от клиента
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.eq_ignore_ascii_case("/quit") {
            break;
        }

        if line.starts_with("###") {
            let username = line[3..].to_string();
            name = Some(username.clone());
            tx.send(format!("System: Welcome, {}! Send messages as: {}: |@username your message", username, username)).await.ok();
            tx.send("System: Type @all ... to broadcast to everyone except you.".to_string()).await.ok();
            println!("[{}] {} joined", peer, username);
            agents.write().await.insert(username.clone(), tx.clone());
            continue;
        }

        if name.is_none() {
            tx.send("Enter your name (#username).".to_string()).await.ok();
            continue;
        }

        // Формат: @target message...
        if let Some((sender, target, body)) = parse_direct_message(line) {
            if body.trim().is_empty() {
                tx.send("Empty message.".to_string()).await.ok();
                continue;
            }

            let current_name = name.as_ref().unwrap();
            if target == "all" {
                broadcast(&agents, current_name, body).await;
            } else {
                let ok = send_to(&agents, current_name, &target, body).await;
                if !ok {
                    tx.send(format!("User '{}' not found.", target)).await.ok();
                }
            }
        } else {
            tx.send("Invalid format. Use: @username message".to_string())
                .await
                .ok();
        }
    }

    // Дерегистрируем (только если был зарегистрирован)
    if let Some(username) = name {
        {
            let mut map = agents.write().await;
            map.remove(&username);
        }
        println!("[{}] '{}' left", peer, username);
    } else {
        println!("[{}] unregistered user left", peer);
    }
    Ok(())
}

fn parse_direct_message(line: &str) -> Option<(String, String, String)> {
    // ожидаем: sender: @name msg...
    let line = line.trim();
    
    // Парсим sender: остальное
    let (sender_part, rest) = line.split_once(": ")?;
    let sender = sender_part.trim().to_string();
    
    // Проверяем, что остальное начинается с @
    if !rest.starts_with('@') {
        return None;
    }
    
    // Убираем @ и парсим target и body
    let rest = &rest[1..];
    let mut it = rest.splitn(2, char::is_whitespace);
    let target = it.next()?.trim().to_string();
    let body = it.next().unwrap_or("").trim().to_string();
    
    if target.is_empty() || sender.is_empty() {
        None
    } else {
        Some((sender, target, body))
    }
}

async fn send_to(agents: &Agents, from: &str, to: &str, body: String) -> bool {
    let map = agents.read().await;
    println!("[{}->{}] {}", from, to, body);
    
    if let Some(tx) = map.get(to) {
        let _ = tx.send(format!("{}: @{} {}", from, to, body)).await;
        true
    } else {
        false
    }
}

async fn broadcast(agents: &Agents, from: &str, body: String) {
    let map = agents.read().await;
    for (name, tx) in map.iter() {
        if name == from {
            continue;
        }
        let _ = tx.send(format!("[Broadcast from {}] {}", from, body)).await;
    }
}
