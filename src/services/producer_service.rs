use crate::models::course::{compare_courses, Course};
use crate::models::deadline::{compare_deadlines, sort_deadlines};
use crate::models::grade::{compare_grades, compare_grades_overview, sort_grades_overview};
use crate::models::notification::Notification;
use crate::models::token::Token;
use crate::models::user::User;
use crate::services::producer_service_interfaces::ProducerServiceInterface;
use crate::services::provider_interfaces::DataProviderInterface;
use anyhow::Result;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use std::sync::Arc;

use super::data_service_interfaces::DataServiceInterfaces;
use super::errors::ServiceError;
use super::event_producer_interface::EventProducerInterface;

pub struct ProducerService {
    producer: Box<dyn EventProducerInterface>,
    data_provider: Arc<dyn DataProviderInterface>,
    data_service: Arc<dyn DataServiceInterfaces>,
}

impl ProducerService {
    pub fn new(
        producer: Box<dyn EventProducerInterface>,
        data_provider: Arc<dyn DataProviderInterface>,
        data_service: Arc<dyn DataServiceInterfaces>,
    ) -> Self {
        Self {
            producer,
            data_provider,
            data_service,
        }
    }
}

#[async_trait]
impl ProducerServiceInterface for ProducerService {
    async fn get_batches<'a>(&self, limit: i64, skip: &'a mut u64) -> Result<()> {
        let mut batch = Vec::new();

        let mut cursor = self.data_service.find_all_tokens(limit, *skip).await?;

        let mut has_documents = false;

        while let Some(doc) = cursor.try_next().await? {
            has_documents = true;
            if let Ok(token) = doc.get_str("_id") {
                match doc.get_str("device_token") {
                    Ok(device_token) => batch.push(Token::new(
                        token.to_string(),
                        Some(device_token.to_string()),
                    )),
                    Err(_) => batch.push(Token::new(token.to_string(), None)),
                };
                *skip += 1;
            }
        }

        // println!("{:?}", batch);

        if !has_documents {
            *skip = 0;
            return Ok(());
        }

        if let Err(e) = self.process_batch(&batch).await {
            eprintln!("Error processing batch: {}", e);
        }
        Ok(())
    }

    async fn process_batch(&self, batch: &[Token]) -> Result<()> {
        for tokens in batch.iter() {
            let token = &tokens.token;

            if let Some(device_token) = &tokens.device_token {
                self.process_producing(token, device_token).await?;
            } else {
                self.data_service.fetch_and_update_data(token).await?;
            }
        }

        Ok(())
    }

    async fn process_producing(&self, token: &str, device_token: &str) -> Result<()> {
        match self.produce_user_info(token, device_token).await {
            Ok(user) => {
                if let Ok(mut courses) = self.produce_course(token, device_token, &user).await {
                    if let Err(e) = self
                        .produce_grade(token, device_token, &user, &courses)
                        .await
                    {
                        eprintln!("Error sending grade: {:?}", e);
                    }
                    if let Err(e) = self
                        .produce_grade_overview(token, device_token, &courses)
                        .await
                    {
                        eprintln!("Error sending grade overview: {:?}", e);
                    }
                    Course::delete_past_courses(&mut courses);
                    if let Err(e) = self.produce_deadline(token, device_token, &courses).await {
                        eprintln!("Error sending deadline: {:?}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error sending user info: {:?}", e);
            }
        }
        Ok(())
    }

    async fn produce_user_info(&self, token: &str, device_token: &str) -> Result<User> {
        let external_user = self.data_provider.get_user(token).await?;
        let user = self.data_service.get_user(token).await?;
        if !user.eq(&external_user) {
            let body = external_user.create_body_message_user();
            let notification =
                Notification::new(device_token.to_string(), "New user info".to_string(), body);
            self.producer.produce_notification(&notification).await;

            self.data_service.update_user(token).await?;
        }
        Ok(external_user)
    }

    async fn produce_course(
        &self,
        token: &str,
        device_token: &str,
        user: &User,
    ) -> Result<Vec<Course>> {
        let mut flag = false;
        let external_courses = self.data_provider.get_courses(token, user.userid).await?;
        let courses = self.data_service.get_courses(token).await?;
        let new_courses = compare_courses(&external_courses, &courses);

        if !new_courses.is_empty() {
            flag = true;

            for new_course in new_courses {
                let body = new_course.fullname.clone();
                let notification =
                    Notification::new(device_token.to_string(), "New course".to_string(), body);
                self.producer.produce_notification(&notification).await;
            }
        }

        if flag {
            self.data_service.update_courses(token, user).await?;
        }
        Ok(external_courses)
    }

    async fn produce_deadline(
        &self,
        token: &str,
        device_token: &str,
        courses: &[Course],
    ) -> Result<()> {
        let mut flag = false;
        for course in courses {
            let deadlines = match self.data_service.get_deadlines(token).await {
                Ok(deadlines) => deadlines,
                Err(e) => match e {
                    ServiceError::DataIsEmpty(_) => vec![],
                    _ => return Err(e.into()),
                },
            };

            let mut external_deadlines = self
                .data_provider
                .get_deadline_by_course_id(token, course.id)
                .await?
                .events;

            if external_deadlines.is_empty() {
                continue;
            };

            for sorted_deadline in external_deadlines.iter_mut() {
                sorted_deadline.coursename = Option::from(course.fullname.clone());
            }

            let sorted_deadlines = sort_deadlines(&mut external_deadlines)?;
            let new_deadlines = compare_deadlines(&sorted_deadlines, &deadlines);

            if !new_deadlines.is_empty() {
                flag = true;
                for new_deadline in new_deadlines {
                    let body = new_deadline.create_body_message_deadline();
                    let notification = Notification::new(
                        device_token.to_string(),
                        "New deadline".to_string(),
                        body,
                    );
                    self.producer.produce_notification(&notification).await;
                }
            }
        }

        if flag {
            self.data_service.update_deadlines(token, courses).await?;
        }

        Ok(())
    }

    async fn produce_grade(
        &self,
        token: &str,
        device_token: &str,
        user: &User,
        courses: &[Course],
    ) -> Result<()> {
        let mut flag = false;
        let past_grades = self.data_service.get_grades(token).await?;

        let all_courses_in_grades = courses
            .iter()
            .all(|course| past_grades.iter().any(|grade| grade.courseid == course.id));

        if !all_courses_in_grades {
            self.data_service
                .update_grades(token, user, courses)
                .await?;
        }

        for course in courses {
            let mut external_grades = self
                .data_provider
                .get_grades_by_course_id(token, user.userid, course.id)
                .await?
                .usergrades;

            for external_grade in external_grades.iter_mut() {
                external_grade.coursename = Option::from(course.fullname.clone());
            }

            let mut grades = self.data_service.get_grades(token).await?;

            for external_grade in external_grades.iter() {
                for grade in grades.iter() {
                    if external_grade.courseid == grade.courseid
                        && external_grade.gradeitems.len() != grade.gradeitems.len()
                    {
                        self.data_service
                            .update_grades(token, user, courses)
                            .await?;
                    }
                }
            }

            let new_grades = compare_grades(&mut external_grades, &mut grades);

            if !new_grades.is_empty() {
                flag = true;
                for new_grade in new_grades {
                    let title = course.fullname.clone();
                    let body = format!(
                        "New grade | {}\n{} -> {}",
                        new_grade.0.itemname,
                        new_grade.1.percentageformatted,
                        new_grade.0.percentageformatted
                    );
                    let notification = Notification::new(device_token.to_string(), title, body);
                    self.producer.produce_notification(&notification).await;
                }
            }
        }
        if flag {
            self.data_service
                .update_grades(token, user, courses)
                .await?;
        }

        Ok(())
    }

    async fn produce_grade_overview(
        &self,
        token: &str,
        device_token: &str,
        courses: &[Course],
    ) -> Result<()> {
        let mut flag = false;
        let external_grades_overview = self
            .data_service
            .fetch_grades_overview(token, courses)
            .await?;

        let mut grades_overview = self.data_service.get_grades_overview(token).await?;
        sort_grades_overview(&mut grades_overview);

        let new_external_grades =
            compare_grades_overview(&external_grades_overview.grades, &grades_overview);
        if !new_external_grades.is_empty() {
            flag = true;
            for new_external_grade in new_external_grades.iter() {
                let title = new_external_grade
                    .course_name
                    .clone()
                    .unwrap_or("-".to_string());
                let body = format!("New course total grade | {}", new_external_grade.grade);
                let notification = Notification::new(device_token.to_string(), title, body);
                self.producer.produce_notification(&notification).await;
            }
        }
        if flag {
            self.data_service
                .update_grades_overview(token, courses)
                .await?;
        }

        Ok(())
    }
}
