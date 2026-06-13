struct NeedsSelf;

#[tork::dependency]
impl NeedsSelf {
    async fn resolve(&self) -> tork::Result<Self> {
        Ok(NeedsSelf)
    }
}

fn main() {}
