use crate::list::model::List;
use async_trait::async_trait;
use mockall::automock;
pub enum SaveError {}

pub enum DeleteError {}
pub enum UpdateError {}
#[derive(Debug)]
pub enum GetListError {}

#[automock]
#[async_trait]
pub trait ListRepo {
    async fn get(&self, id: u64) -> Result<List, GetListError>;
    async fn save(&self) -> Result<u64, SaveError>;
    async fn delete(&self, id: u64) -> Result<(), DeleteError>;
    async fn update(&self, list: &List) -> Result<(), UpdateError>;
}

#[cfg(test)]
mod test {
    use super::*;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_get_list() {
        let mut mock = MockListRepo::new();
        let expected_list = List::default();

        // Set expectation for the async method
        let value = expected_list.clone();
        mock.expect_get()
            .with(eq(1))
            .returning(move |_| Ok(value.clone()));

        // Use the mock
        let result = mock.get(1).await;
        assert_eq!(result.unwrap(), expected_list);
    }
}
