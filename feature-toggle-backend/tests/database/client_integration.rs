use feature_toggle_backend::database::client::{CreateClient, client_repository};
use feature_toggle_backend::database::entity::ClientType;
use feature_toggle_backend::database::init_pg_pool;
use sqlx::PgPool;
use uuid::Uuid;

async fn pool() -> PgPool {
    init_pg_pool().await
}

#[tokio::test]
async fn test_get_clients_seeded() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let list = repo
        .get_clients(team_id, None, None, None)
        .await
        .expect("get_clients ok");

    assert!(list.len() >= 2);
    let web = list
        .iter()
        .find(|c| c.name == "Web Client 1")
        .expect("web client present");
    assert!(matches!(web.client_type, ClientType::Web));
    assert_eq!(web.web_origins.as_ref().map(|v| v.len()).unwrap_or(0), 2);
}

#[tokio::test]
async fn test_get_clients_paginated_with_real_data() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test pagination with page size 1
    let (clients_page1, total) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 1)
        .await
        .expect("get_clients_paginated ok");

    assert_eq!(clients_page1.len(), 1);
    assert!(total >= 2, "Should have at least 2 clients total");

    // Test getting second page
    let (clients_page2, total2) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 2, 1)
        .await
        .expect("get_clients_paginated page 2 ok");

    assert_eq!(clients_page2.len(), 1);
    assert_eq!(total2, total, "Total should be consistent across pages");

    // Ensure pages contain different clients
    assert_ne!(
        clients_page1[0].id, clients_page2[0].id,
        "Different pages should contain different clients"
    );

    // Test larger page size
    let (clients_large_page, total3) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 10)
        .await
        .expect("get_clients_paginated large page ok");

    assert!(clients_large_page.len() >= 2);
    assert_eq!(total3, total, "Total should be consistent");
}

#[tokio::test]
async fn test_get_clients_paginated_with_filters() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test with client type filter
    let (web_clients, web_total) = repo
        .get_clients_paginated(team_id, None, None, Some(ClientType::Web), 1, 10)
        .await
        .expect("get web clients ok");

    assert!(web_total > 0, "Should have at least one web client");
    for client in &web_clients {
        assert!(matches!(client.client_type, ClientType::Web));
    }

    // Test with enabled filter
    let (enabled_clients, enabled_total) = repo
        .get_clients_paginated(team_id, None, Some(true), None, 1, 10)
        .await
        .expect("get enabled clients ok");

    assert!(enabled_total > 0, "Should have at least one enabled client");
    for client in &enabled_clients {
        assert!(client.enabled);
    }
}

#[tokio::test]
async fn test_get_clients_paginated_edge_cases() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test with page number beyond available data
    let (empty_clients, total) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 999, 10)
        .await
        .expect("get_clients_paginated beyond data ok");

    assert_eq!(empty_clients.len(), 0);
    assert!(
        total > 0,
        "Total should still be correct even for empty pages"
    );

    // Test with very large page size
    let (all_clients, total2) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 1000)
        .await
        .expect("get_clients_paginated large page size ok");

    assert_eq!(
        all_clients.len() as i64,
        total2,
        "Should return all available clients"
    );
}

#[tokio::test]
async fn test_create_and_delete_client() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    let name = format!("it-client-{}", Uuid::new_v4());
    let create = CreateClient {
        name: name.clone(),
        description: Some("integ test".into()),
        enabled: true,
        client_type: ClientType::Web,
        web_origins: Some(vec!["http://test.local".into()]),
    };

    let created = repo
        .create_client(team_id, create)
        .await
        .expect("create ok");
    assert_eq!(created.name, name);
    assert_eq!(created.team_id, team_id);
    assert!(matches!(created.client_type, ClientType::Web));
    assert_eq!(created.web_origins.as_ref().unwrap().len(), 1);
    assert_eq!(
        created.web_origins.as_ref().unwrap()[0],
        "http://test.local"
    );
    assert_eq!(created.api_key.len(), 48);

    // cleanup
    repo.delete_client(created.id).await.expect("delete ok");
}

#[tokio::test]
async fn test_pagination_edge_cases_and_boundary_conditions() {
    let pool = pool().await;
    let repo = client_repository(pool);
    let team_id = Uuid::parse_str("51ecc366-f1cd-4d3d-ab73-fa60bad98f27").unwrap();

    // Test page_number = 0 (should be treated as page 1)
    let (clients_page0, total) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 0, 10)
        .await
        .expect("page 0 should work");

    let (clients_page1, total1) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 10)
        .await
        .expect("page 1 should work");

    // Page 0 should behave like page 1
    assert_eq!(clients_page0.len(), clients_page1.len());
    assert_eq!(total, total1);
    if !clients_page0.is_empty() && !clients_page1.is_empty() {
        assert_eq!(clients_page0[0].id, clients_page1[0].id);
    }

    // Test negative page_number (should be treated as page 1)
    let (clients_negative, total_neg) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, -5, 10)
        .await
        .expect("negative page should work");

    assert_eq!(clients_negative.len(), clients_page1.len());
    assert_eq!(total_neg, total);

    // Test page_size = 0 (should return empty results)
    let (clients_zero_size, total_zero) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 0)
        .await
        .expect("zero page size should work");

    assert_eq!(clients_zero_size.len(), 0);
    assert_eq!(total_zero, total); // Total should still be correct

    // Test negative page_size (should return empty results)
    let (clients_neg_size, total_neg_size) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, -5)
        .await
        .expect("negative page size should work");

    assert_eq!(clients_neg_size.len(), 0);
    assert_eq!(total_neg_size, total);

    // Test very large page_size (should return all available results)
    let (clients_large_size, total_large) = repo
        .get_clients_paginated(
            team_id,
            Some("Paginated".to_string()),
            None,
            None,
            1,
            i32::MAX,
        )
        .await
        .expect("very large page size should work");

    assert_eq!(clients_large_size.len() as i64, total_large);
    assert_eq!(total_large, total);

    // Test page number far beyond available data
    let (clients_far_page, total_far) = repo
        .get_clients_paginated(
            team_id,
            Some("Paginated".to_string()),
            None,
            None,
            999999,
            10,
        )
        .await
        .expect("far page should work");

    assert_eq!(clients_far_page.len(), 0);
    assert_eq!(total_far, total);

    // Test with non-existent team (should return empty results)
    let nonexistent_team = Uuid::new_v4();
    let (clients_no_team, total_no_team) = repo
        .get_clients_paginated(
            nonexistent_team,
            Some("Paginated".to_string()),
            None,
            None,
            1,
            10,
        )
        .await
        .expect("non-existent team should work");

    assert_eq!(clients_no_team.len(), 0);
    assert_eq!(total_no_team, 0);

    // Test boundary: page_size = 1, iterate through all pages
    // Get total count without filters for this test
    let (_, total_all_clients) = repo
        .get_clients_paginated(team_id, Some("Paginated".to_string()), None, None, 1, 1)
        .await
        .expect("get total should work");

    if total_all_clients > 0 {
        let mut all_client_ids = std::collections::HashSet::new();
        let total_pages = (total_all_clients + 1 - 1) / 1; // ceil(total / 1)

        for page in 1..=std::cmp::min(total_pages, 5) {
            // Test first 5 pages max
            let (page_clients, page_total) = repo
                .get_clients_paginated(
                    team_id,
                    Some("Paginated".to_string()),
                    None,
                    None,
                    page as i32,
                    1,
                )
                .await
                .expect(&format!("page {} should work", page));

            assert_eq!(
                page_total, total_all_clients,
                "Total should be consistent on page {}",
                page
            );

            if page <= total_all_clients {
                assert_eq!(
                    page_clients.len(),
                    1,
                    "Should have exactly 1 client on page {}",
                    page
                );
                let client_id = page_clients[0].id.clone();
                assert!(
                    !all_client_ids.contains(&client_id),
                    "Client ID should be unique on page {}",
                    page
                );
                all_client_ids.insert(client_id);
            } else {
                assert_eq!(
                    page_clients.len(),
                    0,
                    "Should have no clients beyond total pages"
                );
            }
        }
    }
}
