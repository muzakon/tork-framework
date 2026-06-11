use criterion::{criterion_group, criterion_main, Criterion};
use http_parity::{
    assert_expected_response, http_client, send_request, spawn_server, Backend, Scenario,
};
use tokio::runtime::Runtime;

fn bench_http_core(c: &mut Criterion) {
    let runtime = Runtime::new().expect("criterion runtime");

    for backend in Backend::ALL {
        for scenario in Scenario::ALL {
            let server = runtime
                .block_on(spawn_server(backend, scenario))
                .expect("spawn benchmark server");
            let client = http_client();
            let base_uri = server.base_uri().expect("server base uri");

            let mut group = c.benchmark_group(format!("{backend}/{scenario}"));
            group.bench_function("request", |b| {
                b.to_async(&runtime).iter(|| async {
                    let response = send_request(&client, &base_uri, scenario)
                        .await
                        .expect("send request");
                    assert_expected_response(scenario, &response).expect("assert response");
                });
            });
            group.finish();

            runtime
                .block_on(server.shutdown())
                .expect("shutdown benchmark server");
        }
    }
}

criterion_group!(http_core, bench_http_core);
criterion_main!(http_core);
