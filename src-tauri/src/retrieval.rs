use std::collections::HashSet;

use rusqlite::Connection;
use serde::Serialize;

use crate::storage::Task;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HabitContext {
    #[serde(skip)]
    pub id: i64,
    pub phrase: String,
    pub interpretation: String,
    pub task_id: String,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalStats {
    pub total_tasks: usize,
    pub selected_tasks: usize,
    pub total_habits: usize,
    pub selected_habits: usize,
    pub estimated_context_tokens: usize,
    pub estimated_full_context_tokens: usize,
}

#[derive(Clone, Debug)]
pub struct AiContext {
    pub tasks: Vec<Task>,
    pub habits: Vec<HabitContext>,
    pub stats: RetrievalStats,
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        detail: row.get(2)?,
        progress: row.get::<_, i64>(3)? as u8,
        quadrant: row.get(4)?,
        status: row.get(5)?,
        today: row.get::<_, i64>(6)? != 0,
        planned_date: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(character)
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn search_terms(value: &str) -> Vec<String> {
    let normalized = normalize(value);
    let characters = normalized.chars().collect::<Vec<_>>();
    let mut terms = Vec::new();
    if characters.len() >= 3 {
        for window in characters.windows(3).take(32) {
            let term = window.iter().collect::<String>();
            if !terms.contains(&term) {
                terms.push(term);
            }
        }
    }
    let mut ascii_word = String::new();
    for character in value.chars().chain(std::iter::once(' ')) {
        if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
            ascii_word.push(character.to_ascii_lowercase());
        } else {
            if ascii_word.len() >= 2 && !terms.contains(&ascii_word) {
                terms.push(ascii_word.clone());
            }
            ascii_word.clear();
        }
    }
    terms.truncate(32);
    terms
}

fn fts_query(value: &str) -> Option<String> {
    let terms = search_terms(value);
    (!terms.is_empty()).then(|| {
        terms
            .into_iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ")
    })
}

fn overlap_score(description: &str, candidate: &str) -> usize {
    let description = normalize(description);
    let candidate = normalize(candidate);
    if description.is_empty() || candidate.is_empty() {
        return 0;
    }
    let exact = if description.contains(&candidate) || candidate.contains(&description) {
        100
    } else {
        0
    };
    let description_chars = description.chars().collect::<Vec<_>>();
    let candidate_chars = candidate.chars().collect::<Vec<_>>();
    let description_pairs = description_chars
        .windows(2)
        .map(|pair| pair.iter().collect::<String>())
        .collect::<HashSet<_>>();
    exact
        + candidate_chars
            .windows(2)
            .filter(|pair| description_pairs.contains(&pair.iter().collect::<String>()))
            .count()
}

fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut ascii = 0usize;
    let mut other = 0usize;
    for character in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&character) {
            cjk += 1;
        } else if character.is_ascii_alphanumeric() {
            ascii += 1;
        } else if !character.is_whitespace() {
            other += 1;
        }
    }
    cjk + ascii.div_ceil(4) + other.div_ceil(2) + 4
}

fn task_tokens(task: &Task) -> usize {
    estimate_tokens(&task.id) + estimate_tokens(&task.title) + estimate_tokens(&task.detail) + 12
}

fn habit_tokens(habit: &HabitContext) -> usize {
    estimate_tokens(&habit.phrase)
        + estimate_tokens(&habit.interpretation)
        + estimate_tokens(&habit.task_id)
        + 8
}

fn all_tasks(connection: &Connection) -> Result<Vec<Task>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id,title,detail,progress,quadrant,status,today,planned_date,updated_at
             FROM tasks WHERE status!='已完成'",
        )
        .map_err(|error| error.to_string())?;
    let tasks = statement
        .query_map([], row_to_task)
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(tasks)
}

fn all_habits(connection: &Connection) -> Result<Vec<HabitContext>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id,phrase,interpretation,task_id,kind
             FROM habits WHERE active=1 AND task_id<>''",
        )
        .map_err(|error| error.to_string())?;
    let habits = statement
        .query_map([], |row| {
            Ok(HabitContext {
                id: row.get(0)?,
                phrase: row.get(1)?,
                interpretation: row.get(2)?,
                task_id: row.get(3)?,
                kind: row.get(4)?,
            })
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(habits)
}

fn fts_task_ids(
    connection: &Connection,
    description: &str,
    limit: usize,
) -> Result<Vec<String>, String> {
    let Some(query) = fts_query(description) else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT task_search_v2.task_id
             FROM task_search_v2 JOIN tasks ON tasks.id=task_search_v2.task_id
             WHERE task_search_v2 MATCH ?1 AND tasks.status!='已完成'
             ORDER BY bm25(task_search_v2), tasks.today DESC, tasks.updated_at DESC LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map(rusqlite::params![query, limit as i64], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

fn fts_habit_ids(
    connection: &Connection,
    description: &str,
    limit: usize,
) -> Result<Vec<i64>, String> {
    let Some(query) = fts_query(description) else {
        return Ok(Vec::new());
    };
    let mut statement = connection
        .prepare(
            "SELECT CAST(habit_search.habit_id AS INTEGER)
             FROM habit_search JOIN habits ON habits.id=CAST(habit_search.habit_id AS INTEGER)
             WHERE habit_search MATCH ?1 AND habits.active=1
             ORDER BY bm25(habit_search), habits.updated_at DESC LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map(rusqlite::params![query, limit as i64], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

pub fn retrieve(
    connection: &Connection,
    description: &str,
    task_limit: usize,
    habit_limit: usize,
) -> Result<AiContext, String> {
    let all_tasks = all_tasks(connection)?;
    let all_habits = all_habits(connection)?;

    let fts_ids = fts_task_ids(connection, description, task_limit)?;
    let mut scored_tasks = all_tasks
        .iter()
        .map(|task| {
            let fts_rank = fts_ids
                .iter()
                .position(|id| id == &task.id)
                .map(|rank| 1000usize.saturating_sub(rank));
            let local_score = overlap_score(description, &format!("{}{}", task.title, task.detail));
            (task.clone(), fts_rank.unwrap_or_default() + local_score)
        })
        .collect::<Vec<_>>();
    scored_tasks.sort_by(|(left_task, left_score), (right_task, right_score)| {
        right_score
            .cmp(left_score)
            .then_with(|| right_task.today.cmp(&left_task.today))
            .then_with(|| right_task.updated_at.cmp(&left_task.updated_at))
    });

    let fts_habit_ids = fts_habit_ids(connection, description, habit_limit)?;
    let mut scored_habits = all_habits
        .iter()
        .enumerate()
        .map(|(index, habit)| {
            let fts_rank = fts_habit_ids
                .iter()
                .position(|candidate| *candidate == habit.id)
                .map(|rank| 1000usize.saturating_sub(rank));
            let local_score = overlap_score(description, &habit.phrase);
            (
                habit.clone(),
                fts_rank.unwrap_or_default() + local_score,
                index,
            )
        })
        .filter(|(_, score, _)| *score > 0)
        .collect::<Vec<_>>();
    scored_habits.sort_by(
        |(_, left_score, left_index), (_, right_score, right_index)| {
            right_score
                .cmp(left_score)
                .then_with(|| left_index.cmp(right_index))
        },
    );
    let habits = scored_habits
        .into_iter()
        .take(habit_limit)
        .map(|(habit, _, _)| habit)
        .collect::<Vec<_>>();

    let mut tasks = scored_tasks
        .into_iter()
        .take(task_limit)
        .map(|(task, _)| task)
        .collect::<Vec<_>>();
    for habit in &habits {
        if tasks.iter().any(|task| task.id == habit.task_id) {
            continue;
        }
        if let Some(task) = all_tasks.iter().find(|task| task.id == habit.task_id) {
            if tasks.len() == task_limit && !tasks.is_empty() {
                tasks.pop();
            }
            tasks.insert(0, task.clone());
        }
    }

    let estimated_full_context_tokens = all_tasks.iter().map(task_tokens).sum::<usize>()
        + all_habits.iter().map(habit_tokens).sum::<usize>();
    let estimated_context_tokens = tasks.iter().map(task_tokens).sum::<usize>()
        + habits.iter().map(habit_tokens).sum::<usize>();
    Ok(AiContext {
        stats: RetrievalStats {
            total_tasks: all_tasks.len(),
            selected_tasks: tasks.len(),
            total_habits: all_habits.len(),
            selected_habits: habits.len(),
            estimated_context_tokens,
            estimated_full_context_tokens,
        },
        tasks,
        habits,
    })
}
