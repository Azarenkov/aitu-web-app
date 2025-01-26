use std::error::Error;
use async_trait::async_trait;
use crate::models::user::User;

#[async_trait]
pub trait UserRepositoryInterface: Send + Sync {
    async fn find_by_token(&self, token: &str) -> Result<User, Box<dyn Error>>;
    async fn is_exist(&self, token: &str) -> Result<bool, Box<dyn Error>>;
    async fn save(&self, user: &User, token: &str) -> Result<(), Box<dyn Error>>;
    async fn delete(&self, token: &String) -> Result<(), Box<dyn Error>>;
}