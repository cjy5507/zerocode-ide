use super::{ResultAuthor, ReviewFacts, Run, Task, TaskStatus, same_source};

impl Run {
    #[must_use]
    pub fn completion_ms(&self, task: &Task) -> Option<i64> {
        let earliest = self.completion_floor(task)?;
        match task.result_author.as_ref()? {
            ResultAuthor::Coordinator { completed_ms, .. } => {
                completed_ms.filter(|completed| *completed >= earliest)
            }
            _ => None,
        }
    }

    fn completion_floor(&self, task: &Task) -> Option<i64> {
        if task.status != TaskStatus::Completed {
            return None;
        }
        let review = self.review_of(task);
        if !review.verified || !review.merged || review.source.as_deref()?.trim().is_empty() {
            return None;
        }
        self.dispatches
            .iter()
            .filter(|attempt| attempt.task == task.id)
            .try_fold(task.created_ms.max(0), |earliest, attempt| {
                let ended = attempt.ended_ms?;
                (attempt.started_ms >= 0 && ended >= attempt.started_ms)
                    .then_some(earliest.max(ended))
            })
    }

    pub(super) fn stamp_completion(
        &mut self,
        task_index: usize,
        previous_review: ReviewFacts,
        previous_completed: Option<i64>,
        wrote_result: bool,
        now_ms: i64,
    ) {
        let task = &self.tasks[task_index];
        let review = self.review_of(task);
        let same_review = review.attempt == previous_review.attempt
            && review
                .source
                .as_deref()
                .zip(previous_review.source.as_deref())
                .is_some_and(|(current, previous)| same_source(current, previous));
        let completed = self.completion_floor(task).and_then(|earliest| {
            previous_completed
                .filter(|_| same_review)
                .or_else(|| (wrote_result && now_ms >= earliest).then_some(now_ms))
        });
        if let Some(ResultAuthor::Coordinator { completed_ms, .. }) =
            &mut self.tasks[task_index].result_author
        {
            *completed_ms = completed;
        }
    }
}
