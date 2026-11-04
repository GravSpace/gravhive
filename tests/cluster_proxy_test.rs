use gravhive::cluster_proxy::ClusterLoadBalancer;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn test_cluster_l4_load_balancer_round_robin() {
    // 1. Start Server 1
    let listener_1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_1 = listener_1.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener_1.accept().await {
            let _ = stream.write_all(b"HELLO_FROM_1\n").await;
        }
    });

    // 2. Start Server 2
    let listener_2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_2 = listener_2.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener_2.accept().await {
            let _ = stream.write_all(b"HELLO_FROM_2\n").await;
        }
    });

    // 3. Start ClusterLoadBalancer pointing to [port_1, port_2]
    let balancer = ClusterLoadBalancer::start(
        "test-api".to_string(),
        0, // Bind to dynamic OS port
        vec![port_1, port_2],
    )
    .await
    .expect("Failed to start cluster load balancer");

    let lb_port = balancer.listen_port;
    assert!(lb_port > 0);
    assert_eq!(balancer.get_upstreams().await, vec![port_1, port_2]);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Helper to send request and read response
    async fn fetch_response(port: u16) -> String {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .expect("Failed to connect to load balancer");
        let mut buf = vec![0u8; 64];
        let n = stream.read(&mut buf).await.unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).trim().to_string()
    }

    // 4. Test Round-Robin distribution across 4 requests
    let resp1 = fetch_response(lb_port).await;
    let resp2 = fetch_response(lb_port).await;
    let resp3 = fetch_response(lb_port).await;
    let resp4 = fetch_response(lb_port).await;

    assert_eq!(resp1, "HELLO_FROM_1");
    assert_eq!(resp2, "HELLO_FROM_2");
    assert_eq!(resp3, "HELLO_FROM_1");
    assert_eq!(resp4, "HELLO_FROM_2");

    // 5. Test dynamic updating of upstream workers (draining Server 1)
    balancer.update_upstreams(vec![port_2]).await;
    assert_eq!(balancer.get_upstreams().await, vec![port_2]);

    let resp5 = fetch_response(lb_port).await;
    let resp6 = fetch_response(lb_port).await;
    assert_eq!(resp5, "HELLO_FROM_2");
    assert_eq!(resp6, "HELLO_FROM_2");

    // 6. Test shutdown
    balancer.shutdown();
}
