struct RequestPath(String);

#[tork::dependency]
impl RequestPath {
    async fn resolve() -> tork::Result<Self> {
        Ok(Self("/".to_owned()))
    }
}

fn main() {
    let _ = RequestPath::resolve;
}
