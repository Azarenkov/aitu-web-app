use std::error::Error;
use std::sync::Arc;
use async_trait::async_trait;
use crate::models::course::course_model::Course;
use crate::models::grade::grade_model::Grade;
use crate::models::user::user_model::User;
use crate::services::interfaces::grade_service_interface::GradeServiceInteface;
use crate::services::interfaces::provider_interface::ProviderInterface;
use crate::services::repositories::grade_repository_interface::GradeRepositoryInterface;

pub struct GradeService  {
    grade_repository: Arc<dyn GradeRepositoryInterface>,
    grade_provider: Arc<dyn ProviderInterface>,
}

impl GradeService {
    pub fn new(grade_repository: Arc<dyn GradeRepositoryInterface>, grade_provider: Arc<dyn ProviderInterface>) -> Self {
        Self { grade_repository, grade_provider }
    }
}

#[async_trait(?Send)]
impl GradeServiceInteface for GradeService {
    async fn get_grades(&self, token: &str) -> Result<Vec<Grade>, Box<dyn Error>> {
        self.grade_repository.find_by_token(token).await
    }

    async fn update_grades(&self, token: &str, user: &User, courses: &[Course]) -> Result<(), Box<dyn Error>> {
        let mut grades = Vec::new();
        
        for course in courses {
            let external_grades = self.grade_provider.get_grades_by_course_id(token, user.userid, course.id).await?.usergrades;
            for grade in external_grades {
                grades.push(grade);
            }
        }
        
        self.grade_repository.save(token, &grades).await?;
        Ok(())
    }
}