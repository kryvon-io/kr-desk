//! Testes do cliente contra um servidor WebSocket simulado.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use kr_identity::DeviceIdentity;
use kr_proto::{ClientMessage, Envelope, ServerMessage};
use kr_signaling::{ConnectionStatus, SignalingConfig, SignalingEvent, spawn};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

/// O que o servidor simulado reporta ao teste.
#[derive(Debug)]
enum Seen {
    Auth { conn: usize, public_key: String },
    Msg { conn: usize, msg: ClientMessage },
    Closed(usize),
}

/// Ações que o teste manda o servidor executar numa conexão.
enum Action {
    Send(ServerMessage),
    Drop,
}

struct MockServer {
    url: String,
    seen: mpsc::UnboundedReceiver<Seen>,
    conns: Arc<tokio::sync::Mutex<Vec<mpsc::UnboundedSender<Action>>>>,
}

impl MockServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let (seen_tx, seen) = mpsc::unbounded_channel();
        let conns: Arc<tokio::sync::Mutex<Vec<mpsc::UnboundedSender<Action>>>> = Default::default();
        let conns2 = conns.clone();

        tokio::spawn(async move {
            let mut n = 0usize;
            while let Ok((tcp, _)) = listener.accept().await {
                let id = n;
                n += 1;
                let seen_tx = seen_tx.clone();
                let (act_tx, mut act_rx) = mpsc::unbounded_channel::<Action>();
                conns2.lock().await.push(act_tx);
                tokio::spawn(async move {
                    let ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
                    let (mut sink, mut stream) = ws.split();
                    let out = |m: ServerMessage| {
                        Message::text(serde_json::to_string(&Envelope::new(m)).unwrap())
                    };
                    sink.send(out(ServerMessage::Hello {
                        nonce: format!("nonce-{id}"),
                        server_time_ms: 0,
                    }))
                    .await
                    .unwrap();
                    loop {
                        tokio::select! {
                            frame = stream.next() => match frame {
                                Some(Ok(Message::Text(t))) => {
                                    let env: Envelope<ClientMessage> = serde_json::from_str(&t).unwrap();
                                    match env.msg {
                                        ClientMessage::Auth { public_key, .. } => {
                                            let _ = seen_tx.send(Seen::Auth { conn: id, public_key });
                                            sink.send(out(ServerMessage::AuthOk { device_id: "123456789".into() })).await.unwrap();
                                        }
                                        ClientMessage::Ping { nonce } => {
                                            let _ = seen_tx.send(Seen::Msg { conn: id, msg: ClientMessage::Ping { nonce } });
                                            let _ = sink.send(out(ServerMessage::Pong { nonce })).await;
                                        }
                                        msg => { let _ = seen_tx.send(Seen::Msg { conn: id, msg }); }
                                    }
                                }
                                Some(Ok(_)) => {}
                                _ => { let _ = seen_tx.send(Seen::Closed(id)); break; }
                            },
                            act = act_rx.recv() => match act {
                                Some(Action::Send(m)) => { let _ = sink.send(out(m)).await; }
                                Some(Action::Drop) | None => { let _ = sink.close().await; let _ = seen_tx.send(Seen::Closed(id)); break; }
                            },
                        }
                    }
                });
            }
        });
        Self { url, seen, conns }
    }

    async fn act(&self, conn: usize, action: Action) {
        self.conns.lock().await[conn].send(action).ok();
    }

    /// Espera um evento que satisfaça `pred`, ignorando os demais.
    async fn expect(&mut self, what: &str, pred: impl Fn(&Seen) -> bool) -> Seen {
        timeout(Duration::from_secs(5), async {
            loop {
                let s = self.seen.recv().await.expect("servidor encerrou");
                if pred(&s) {
                    return s;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timeout esperando {what}"))
    }
}

fn config(url: &str) -> SignalingConfig {
    let mut c = SignalingConfig::new(url, "PC de teste", "0.0.0-test");
    c.heartbeat = Duration::from_millis(200);
    c.backoff_min = Duration::from_millis(50);
    c.backoff_max = Duration::from_millis(200);
    c
}

async fn next_event(
    rx: &mut mpsc::UnboundedReceiver<SignalingEvent>,
    pred: impl Fn(&SignalingEvent) -> bool,
) -> SignalingEvent {
    timeout(Duration::from_secs(5), async {
        loop {
            let ev = rx.recv().await.expect("cliente encerrou");
            if pred(&ev) {
                return ev;
            }
        }
    })
    .await
    .expect("timeout esperando evento")
}

fn is_online(ev: &SignalingEvent) -> bool {
    matches!(ev, SignalingEvent::Status(ConnectionStatus::Online { .. }))
}

#[tokio::test]
async fn autentica_recebe_mensagens_envia_e_faz_heartbeat() {
    let mut server = MockServer::start().await;
    let identity = Arc::new(DeviceIdentity::generate().unwrap());
    let (handle, mut events) = spawn(config(&server.url), identity.clone());

    let Seen::Auth { public_key, .. } = server
        .expect("auth", |s| matches!(s, Seen::Auth { .. }))
        .await
    else {
        unreachable!()
    };
    assert_eq!(public_key, identity.public_key_b64());
    let ev = next_event(&mut events, is_online).await;
    assert_eq!(
        ev,
        SignalingEvent::Status(ConnectionStatus::Online {
            device_id: "123456789".into()
        })
    );
    assert!(handle.is_online());

    // servidor → app
    server
        .act(
            0,
            Action::Send(ServerMessage::ConnectPending {
                request_id: "r1".into(),
            }),
        )
        .await;
    let ev = next_event(&mut events, |e| matches!(e, SignalingEvent::Message(_))).await;
    assert_eq!(
        ev,
        SignalingEvent::Message(ServerMessage::ConnectPending {
            request_id: "r1".into()
        })
    );

    // app → servidor
    handle
        .send(ClientMessage::EndSession {
            request_id: "r1".into(),
        })
        .unwrap();
    server
        .expect("end_session", |s| {
            matches!(
                s,
                Seen::Msg {
                    msg: ClientMessage::EndSession { .. },
                    ..
                }
            )
        })
        .await;

    // heartbeat
    server
        .expect("ping", |s| {
            matches!(
                s,
                Seen::Msg {
                    msg: ClientMessage::Ping { .. },
                    ..
                }
            )
        })
        .await;

    handle.shutdown();
    server
        .expect("fechamento", |s| matches!(s, Seen::Closed(0)))
        .await;
}

#[tokio::test]
async fn rotacao_abre_nova_conexao_antes_de_fechar_a_antiga() {
    let mut server = MockServer::start().await;
    let (handle, mut events) = spawn(
        config(&server.url),
        Arc::new(DeviceIdentity::generate().unwrap()),
    );
    next_event(&mut events, is_online).await;

    server
        .act(
            0,
            Action::Send(ServerMessage::ReconnectSoon { in_ms: 30_000 }),
        )
        .await;
    server
        .expect("auth na conexão 1", |s| {
            matches!(s, Seen::Auth { conn: 1, .. })
        })
        .await;
    server
        .expect("antiga fechada", |s| matches!(s, Seen::Closed(0)))
        .await;

    // A nova conexão é usada para enviar; nenhum Offline foi emitido.
    handle
        .send(ClientMessage::EndSession {
            request_id: "x".into(),
        })
        .unwrap();
    server
        .expect("mensagem na conexão nova", |s| {
            matches!(
                s,
                Seen::Msg {
                    conn: 1,
                    msg: ClientMessage::EndSession { .. }
                }
            )
        })
        .await;
    while let Ok(ev) = events.try_recv() {
        assert!(
            !matches!(ev, SignalingEvent::Status(ConnectionStatus::Offline { .. })),
            "não deveria ficar offline na rotação: {ev:?}"
        );
    }
    handle.shutdown();
}

#[tokio::test]
async fn reconecta_quando_o_servidor_derruba() {
    let mut server = MockServer::start().await;
    let (handle, mut events) = spawn(
        config(&server.url),
        Arc::new(DeviceIdentity::generate().unwrap()),
    );
    next_event(&mut events, is_online).await;

    server.act(0, Action::Drop).await;
    next_event(&mut events, |e| {
        matches!(e, SignalingEvent::Status(ConnectionStatus::Offline { .. }))
    })
    .await;
    assert!(matches!(
        handle.send(ClientMessage::Ping { nonce: 1 }),
        Err(kr_signaling::SignalingError::NotConnected)
    ));
    server
        .expect("reconexão", |s| matches!(s, Seen::Auth { conn: 1, .. }))
        .await;
    next_event(&mut events, is_online).await;
    assert!(handle.is_online());
    handle.shutdown();
}

#[tokio::test]
async fn fica_tentando_quando_o_servidor_nao_existe() {
    // Porta fechada: deve emitir Offline com tempo de nova tentativa.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    drop(listener);
    let (handle, mut events) = spawn(config(&url), Arc::new(DeviceIdentity::generate().unwrap()));
    let ev = next_event(&mut events, |e| {
        matches!(e, SignalingEvent::Status(ConnectionStatus::Offline { .. }))
    })
    .await;
    let SignalingEvent::Status(ConnectionStatus::Offline { retry_in_ms, .. }) = ev else {
        unreachable!()
    };
    assert!(retry_in_ms <= 200);
    handle.shutdown();
}
