use crate::models::course::Course;
use crate::models::deadline::{sort_deadlines, Deadline};
use crate::models::errors::ApiError;
use crate::models::grade::{sort_grades_overview, Grade, GradeOverview, GradesOverview};
use crate::models::token::Token;
use crate::models::user::User;
use crate::services::data_service_interfaces::CourseServiceInterface;
use crate::services::data_service_interfaces::DeadlineServiceInterface;
use crate::services::data_service_interfaces::GradeServiceInterface;
use crate::services::data_service_interfaces::TokenServiceInterface;
use crate::services::data_service_interfaces::UserServiceInterface;
use crate::services::provider_interfaces::DataProviderInterface;
use anyhow::Error;
use anyhow::Result;
use async_trait::async_trait;
use mongodb::bson::Document;
use mongodb::Cursor;
use std::sync::Arc;

use super::data_service_interfaces::DataServiceInterfaces;

#[async_trait]
pub trait RepositoryInterfaces:
    TokenRepositoryInterface
    + UserRepositoryInterface
    + CourseRepositoryInterface
    + DeadlineRepositoryInterface
    + GradeRepositoryInterface
    + Send
    + Sync
{
}

#[async_trait]
pub trait TokenRepositoryInterface {
    async fn save_tokens(&self, token: &Token) -> Result<()>;
    async fn find_all_device_tokens(&self, limit: i64, skip: u64) -> Result<Cursor<Document>>;
    async fn delete(&self, token: &str) -> Result<()>;
}

#[async_trait]
pub trait UserRepositoryInterface {
    async fn find_user_by_token(&self, token: &str) -> Result<User>;
    async fn save_user(&self, user: &User, token: &str) -> Result<()>;
}

#[async_trait]
pub trait CourseRepositoryInterface {
    async fn save_courses(&self, token: &str, courses: &[Course]) -> Result<()>;
    async fn find_courses_by_token(&self, token: &str) -> Result<Vec<Course>>;
}

#[async_trait]
pub trait DeadlineRepositoryInterface {
    async fn save_deadlines(&self, token: &str, deadlines: &[Deadline]) -> Result<()>;
    async fn find_deadlines_by_token(&self, token: &str) -> Result<Vec<Deadline>>;
}

#[async_trait]
pub trait GradeRepositoryInterface {
    async fn save_grades(&self, token: &str, grades: &[Grade]) -> Result<()>;
    async fn find_grades_by_token(&self, token: &str) -> Result<Vec<Grade>>;
    async fn save_grades_overview(
        &self,
        token: &str,
        grades_overview: &GradesOverview,
    ) -> Result<()>;
    async fn find_grades_overview_by_token(&self, token: &str) -> Result<Vec<GradeOverview>>;
}

pub struct DataService {
    data_provider: Arc<dyn DataProviderInterface>,
    data_repositories: Box<dyn RepositoryInterfaces>,
}

impl DataService {
    pub fn new(
        data_provider: Arc<dyn DataProviderInterface>,
        data_repositories: Box<dyn RepositoryInterfaces>,
    ) -> Self {
        Self {
            data_provider,
            data_repositories,
        }
    }
}
#[async_trait]
impl DataServiceInterfaces for DataService {}

#[async_trait]
impl TokenServiceInterface for DataService {
    async fn create_token(&self, token: &Token) -> Result<()> {
        match self.data_provider.valid_token(&token.token).await {
            Ok(_) => {}
            Err(_) => return Err(Error::new(ApiError::InvalidToken)),
        };

        match self.data_repositories.save_tokens(token).await {
            Ok(_) => Ok(()),
            Err(e) => match e.downcast_ref::<ApiError>() {
                Some(ApiError::UserAlreadyExists) => Err(Error::new(ApiError::UserAlreadyExists)),
                _ => Err(Error::new(ApiError::InternalServerError)),
            },
        }
    }

    async fn delete_one_user(&self, token: &str) -> Result<()> {
        self.data_repositories.delete(token).await
    }

    async fn find_all_tokens(&self, limit: i64, skip: u64) -> Result<Cursor<Document>> {
        self.data_repositories
            .find_all_device_tokens(limit, skip)
            .await
    }

    async fn fetch_and_save_data(&self, token: &str) -> Result<()> {
        let user = self.create_user(token).await?;
        let courses = self.update_courses(token, &user).await?;
        self.update_grades(token, &user, &courses).await?;
        self.update_grades_overview(token, &courses).await?;
        self.update_deadlines(token, &courses).await?;
        Ok(())
    }
}

#[async_trait]
impl UserServiceInterface for DataService {
    async fn create_user(&self, token: &str) -> Result<User> {
        match self.data_provider.get_user(token).await {
            Ok(user) => {
                self.data_repositories.save_user(&user, token).await?;
                Ok(user)
            }
            Err(_) => Err(Error::new(ApiError::InvalidToken)),
        }
    }

    async fn get_user(&self, token: &str) -> Result<User> {
        self.data_repositories.find_user_by_token(token).await
    }
}

#[async_trait]
impl CourseServiceInterface for DataService {
    async fn get_courses(&self, token: &str) -> Result<Vec<Course>> {
        self.data_repositories.find_courses_by_token(token).await
    }

    async fn update_courses(&self, token: &str, user: &User) -> Result<Vec<Course>> {
        let courses = self.data_provider.get_courses(token, user.userid).await?;
        self.data_repositories.save_courses(token, &courses).await?;
        Ok(courses)
    }
}

#[async_trait]
impl GradeServiceInterface for DataService {
    async fn get_grades(&self, token: &str) -> Result<Vec<Grade>> {
        self.data_repositories.find_grades_by_token(token).await
    }

    async fn update_grades(&self, token: &str, user: &User, courses: &[Course]) -> Result<()> {
        let mut grades = Vec::new();

        for course in courses {
            let external_grades = self
                .data_provider
                .get_grades_by_course_id(token, user.userid, course.id)
                .await?
                .usergrades;
            for mut grade in external_grades {
                grade.coursename = Option::from(course.fullname.clone());
                grades.push(grade);
            }
        }

        self.data_repositories.save_grades(token, &grades).await?;
        Ok(())
    }

    async fn get_grades_overview(&self, token: &str) -> Result<Vec<GradeOverview>> {
        self.data_repositories
            .find_grades_overview_by_token(token)
            .await
    }

    async fn update_grades_overview(&self, token: &str, courses: &[Course]) -> Result<()> {
        let mut grades_overview = self.data_provider.get_grades_overview(token).await?;

        for grade_overview in grades_overview.grades.iter_mut() {
            for course in courses {
                if course.id == grade_overview.courseid {
                    grade_overview.course_name = Option::from(course.fullname.clone());
                    break;
                }
            }
        }
        sort_grades_overview(&mut grades_overview.grades);
        self.data_repositories
            .save_grades_overview(token, &grades_overview)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl DeadlineServiceInterface for DataService {
    async fn get_deadlines(&self, token: &str) -> Result<Vec<Deadline>> {
        self.data_repositories.find_deadlines_by_token(token).await
    }

    async fn update_deadlines(&self, token: &str, courses: &[Course]) -> Result<()> {
        let mut deadlines = Vec::new();

        for course in courses {
            let external_deadlines = self
                .data_provider
                .get_deadline_by_course_id(token, course.id)
                .await?
                .events;
            for mut deadline in external_deadlines {
                deadline.coursename = Option::from(course.fullname.clone());
                deadlines.push(deadline);
            }
        }
        let sorted_deadlines = sort_deadlines(&mut deadlines)?;
        self.data_repositories
            .save_deadlines(token, &sorted_deadlines)
            .await?;
        Ok(())
    }
}
