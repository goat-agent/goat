use std::fmt::Write as _;

use goat_protocol::{SkillArgument, SkillArgumentValue, SkillChoice, SkillInfo};
use goat_provider::ContentBlock;
use goat_skill::{Argument, ArgumentValue, Scopes, Skill, SkillSet};

pub(crate) const PRINCIPLES: &str = concat!(
    "You are Goat, a software-engineering agent working in a terminal workspace. ",
    "You act through tools and speak to the user through a transcript.\n\n",
    "- Do what the request asks and respect project conventions; surface blocking constraints or ambiguity instead of guessing, and ask the user when a choice is material and you cannot settle it from the workspace.\n",
    "- Ground every claim in files, tool output, or cited sources; never invent code, paths, results, or citations, and say so when you are unsure.\n",
    "- When you work with an external library, framework, or API, treat this project's actual code, configuration, and tool output as authoritative over your trained memory of it; your memory of fast-moving surfaces may be stale or version-skewed, and you may not feel uncertain when it is, so mirror what the project already does rather than what you remember.\n",
    "- Prefer targeted inspection over broad reading; understand code before changing it, and fix the underlying cause rather than the surface symptom. Reach for what the project, standard library, or platform already provides before adding new code, dependencies, or abstractions; keep changes minimal and consistent with the surrounding code, and leave unrelated lines untouched.\n",
    "- Build only what the request needs: question whether a requirement, option, or layer should exist before adding it, but solve the user's actual goal in full rather than substituting a reduced or staged version — and never drop validation, security, or data-safety to get there.\n",
    "- Verify your work when a check is available and confirm it actually holds before claiming it is done; then report plainly and no longer than the task needs what you did, how you know it holds, and any remaining risks or next steps.\n",
    "- Reply to the user in their language, but keep code, identifiers, paths, commands, tool arguments, and quoted excerpts verbatim; write text stored in the repository (commit messages, comments, PR descriptions) in the project's prevailing language."
);

pub(crate) const LANGUAGE_REMINDER: &str = "[Reminder: write your prose to the user in the language they used in their request. Keep code, identifiers, file paths, shell commands, tool arguments, and quoted file or output excerpts exactly as they are. Text stored in the repository stays in the project's prevailing language.]";

pub(crate) fn language_anchor_block() -> ContentBlock {
    ContentBlock::Text {
        text: LANGUAGE_REMINDER.to_owned(),
    }
}

pub(crate) fn append_language_anchor(
    mut content: Vec<ContentBlock>,
    is_top: bool,
) -> Vec<ContentBlock> {
    if is_top {
        content.push(language_anchor_block());
    }
    content
}

fn env_segment(cwd: &std::path::Path, os: &str, date: &str) -> String {
    format!(
        "\n\n# Environment\n\n- date: {date}\n- cwd: {}\n- os: {os}",
        cwd.display()
    )
}

pub(crate) fn current_utc_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let (year, month, day) = civil_date_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

pub(crate) fn build_system_prompt(
    cwd: &std::path::Path,
    skills: &SkillSet,
    instructions: Option<&str>,
    date: &str,
    plan: Option<&std::path::Path>,
) -> String {
    let mut prompt = String::from(PRINCIPLES);
    prompt.push_str(&env_segment(cwd, std::env::consts::OS, date));
    if let Some(catalog) = skills.catalog() {
        let _ = write!(prompt, "\n\n# Skills\n\n{catalog}");
    }
    if let Some(content) = instructions {
        let _ = write!(prompt, "\n\n{content}");
    }
    if let Some(path) = plan {
        prompt.push_str(&plan_segment(path));
    }
    prompt
}

pub(crate) fn plan_segment(path: &std::path::Path) -> String {
    format!(
        concat!(
            "\n\n# 계획 모드\n\n",
            "지금은 계획 단계다. 목표는 실행 전에 \"끝\"을 정의하는 것이다.\n",
            "플랜은 {} 에 쓴다. 없으면 새로 만든다.\n\n",
            "## 순서\n",
            "1. 코드를 먼저 읽어라. 저장소를 보면 알 수 있는 것은 절대 사용자에게 묻지 않는다.\n",
            "2. 그러고도 남는 것 중 사용자만 답할 수 있는 것만 Ask로 묻는다.\n",
            "3. 플랜을 쓴다.\n",
            "4. ProposePlan을 부른다.\n\n",
            "## 질문\n",
            "- 한 번에 하나만 묻는다.\n",
            "- 선택지를 2개 이상 제시하고 추천을 표시한다. 하나만 제시하면 사용자는 그냥 승인해버린다.\n",
            "- 답이 없으면 가정으로 기록하고 진행한다. 막히지 마라.\n",
            "- 되돌릴 수 없는 것(데이터 삭제, 프로덕션, 외부로 나가는 쓰기)은 가정하지 말고 반드시 묻는다.\n\n",
            "## 추측 금지\n",
            "확인한 것과 가정한 것을 섞지 마라.\n",
            "- 코드에서 확인한 주장에는 파일 경로를 댄다. 못 대면 그것은 가정이다.\n",
            "- 가정은 한곳에 모아라. 사용자가 승인 화면에서 읽는 것이 그 목록이다.\n",
            "- 답이 필요해 막힌 것은 따로 적어라.\n\n",
            "## 완료 조건\n",
            "검사할 수 없는 문장은 조건이 아니다.\n",
            "  나쁨: \"플랜 모드가 잘 동작해야 함\"\n",
            "  좋음: \"`cargo nextest run -p goat-engine plan::` 가 통과한다\"\n",
            "명령·경로·구체적 문자열을 그대로 적어라.\n",
            "작업이 크면 조건 개수를 늘리지 말고 층을 나눠라. 맨 위 조건도 검사 가능해야 한다.\n\n",
            "## 제약\n",
            "계획 중에도 도구는 전부 쓸 수 있다. 파일을 읽고, 문서를 쓰고, 테스트를 돌려도 된다.\n",
            "막히는 것은 없다. 다만 \"일을 하는 것\"과 \"일을 계획하는 것\"을 구분하라 — ",
            "구현을 시작하지는 마라."
        ),
        path.display()
    )
}

pub(crate) fn compose_child_system(base_prompt: &str, instructions: Option<&str>) -> String {
    let mut prompt = format!("{PRINCIPLES}\n\n{base_prompt}");
    if let Some(content) = instructions {
        let _ = write!(prompt, "\n\n{content}");
    }
    prompt
}

pub(crate) fn load_skills(cwd: &std::path::Path) -> SkillSet {
    let Some(root) = goat_config::root() else {
        return SkillSet::default();
    };
    SkillSet::load(&Scopes::code(root, cwd))
}

pub(crate) fn skill_info(skill: &Skill) -> SkillInfo {
    SkillInfo {
        name: skill.name.clone(),
        description: skill.description.clone(),
        arguments: skill.arguments.iter().map(skill_argument).collect(),
    }
}

fn skill_argument(argument: &Argument) -> SkillArgument {
    SkillArgument {
        name: argument.name.clone(),
        description: argument.description.clone(),
        required: argument.required,
        value: match &argument.value {
            ArgumentValue::Word => SkillArgumentValue::Word {},
            ArgumentValue::Integer => SkillArgumentValue::Integer {},
            ArgumentValue::TextTail => SkillArgumentValue::TextTail {},
            ArgumentValue::Choice(options) => SkillArgumentValue::Choice {
                options: options
                    .iter()
                    .map(|option| SkillChoice {
                        value: option.value.clone(),
                        description: option.description.clone(),
                    })
                    .collect(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{Scopes, SkillSet};
    use std::path::Path;

    fn set_with_demo(root: &Path) -> SkillSet {
        let dir = root.join("skills/demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: demo\ndescription: does the demo\n---\nthe demo body",
        )
        .unwrap();
        SkillSet::load(&Scopes::code(root, root).with_agents_user(None))
    }

    #[test]
    fn system_prompt_starts_with_principles_and_lists_environment() {
        let prompt = super::build_system_prompt(
            Path::new("/work/project"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(prompt.starts_with(super::PRINCIPLES));
        assert!(prompt.contains("# Environment"));
        assert!(prompt.contains("cwd: /work/project"));
        assert!(prompt.contains(&format!("os: {}", std::env::consts::OS)));
        assert!(!prompt.contains("# Skills"));
    }

    #[test]
    fn env_block_lists_date_cwd_and_os() {
        let segment = super::env_segment(Path::new("/tmp/here"), "linux", "2025-01-15");
        assert!(segment.contains("# Environment"));
        assert!(segment.contains("- date: 2025-01-15"));
        assert!(segment.contains("- cwd: /tmp/here"));
        assert!(segment.contains("- os: linux"));
    }

    #[test]
    fn current_utc_date_is_iso_formatted() {
        let date = super::current_utc_date();
        let bytes = date.as_bytes();
        assert_eq!(date.len(), 10);
        assert_eq!(bytes[4], b'-');
        assert_eq!(bytes[7], b'-');
        assert!(date[0..4].chars().all(|c| c.is_ascii_digit()));
        assert!(date[5..7].chars().all(|c| c.is_ascii_digit()));
        assert!(date[8..10].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn civil_date_matches_known_unix_days() {
        assert_eq!(super::civil_date_from_unix_days(0), (1970, 1, 1));
        assert_eq!(super::civil_date_from_unix_days(19_723), (2024, 1, 1));
        assert_eq!(super::civil_date_from_unix_days(20_134), (2025, 2, 15));
    }

    #[test]
    fn system_prompt_carries_authority_principle() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(prompt.contains("authoritative over your trained memory"));
        assert!(prompt.contains("mirror what the project already does"));
    }

    #[test]
    fn system_prompt_lists_skills() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &set_with_demo(dir.path()),
            None,
            "2025-01-15",
            None,
        );
        assert!(prompt.contains("# Skills"));
        assert!(prompt.contains("<name>demo</name>"));
        assert!(prompt.contains("does the demo"));
        assert!(prompt.contains("`Skill` tool"));
        assert!(
            !prompt.contains("the demo body"),
            "the catalog announces a skill; the Skill tool delivers it"
        );
    }

    #[test]
    fn plan_segment_is_absent_outside_plan_mode() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(!prompt.contains("계획 모드"));
        assert!(!prompt.contains("ProposePlan"));
    }

    #[test]
    fn plan_segment_names_the_plan_file_and_lands_last() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            Some("# Project instructions (repo/AGENTS.md)\n\nrule"),
            "2025-01-15",
            Some(Path::new("/plans/1-demo.md")),
        );
        assert!(prompt.contains("/plans/1-demo.md"));
        assert!(prompt.contains("ProposePlan"));
        let instructions = prompt.find("# Project instructions").unwrap();
        let plan = prompt.find("# 계획 모드").unwrap();
        assert!(
            instructions < plan,
            "the plan segment must come after project instructions so the mode banner is the last word"
        );
    }

    #[test]
    fn plan_segment_permits_tools_and_forbids_implementing() {
        let prompt = super::plan_segment(Path::new("/plans/1-demo.md"));
        assert!(prompt.contains("도구는 전부 쓸 수 있다"));
        assert!(prompt.contains("구현을 시작하지는 마라"));
        assert!(prompt.contains("선택지를 2개 이상"));
    }

    #[test]
    fn system_prompt_includes_project_instructions() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            Some("always use snake_case"),
            "2025-01-15",
            None,
        );
        assert!(prompt.contains("always use snake_case"));
    }

    #[test]
    fn system_prompt_no_instructions_omits_section() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(!prompt.contains("Project instructions"));
    }

    #[test]
    fn system_prompt_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (repo/AGENTS.md)";
        let instructions = format!("{heading}\n\nalways use snake_case");
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            Some(&instructions),
            "2025-01-15",
            None,
        );
        assert_eq!(prompt.matches(heading).count(), 1);
        assert!(prompt.ends_with(&instructions));
        assert!(!prompt.contains("# Project instructions (AGENTS.md)\n\n# Project instructions"));
    }

    #[test]
    fn child_system_appends_project_instructions_verbatim() {
        let heading = "# Project instructions (x)";
        let instructions = format!("{heading}\n\nrule");
        let prompt = super::compose_child_system("child base", Some(&instructions));
        assert_eq!(prompt.matches(heading).count(), 1);
        assert!(prompt.contains("child base"));
        assert!(prompt.ends_with(&instructions));
        assert!(!prompt.contains("# Project instructions (AGENTS.md)\n\n# Project instructions"));
    }

    #[test]
    fn child_system_carries_shared_principles() {
        let with_instructions = super::compose_child_system("child base", Some("rule"));
        assert!(with_instructions.starts_with(super::PRINCIPLES));
        assert!(with_instructions.contains("child base"));
        let without = super::compose_child_system("child base", None);
        assert!(without.starts_with(super::PRINCIPLES));
        assert!(without.ends_with("child base"));
    }

    #[test]
    fn principles_carry_build_discipline() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(prompt.contains("Build only what the request needs"));
        assert!(prompt.contains("reduced or staged version"));
        assert!(prompt.contains("never drop validation, security, or data-safety"));
        assert!(prompt.contains("fix the underlying cause rather than the surface symptom"));
    }

    #[test]
    fn system_prompt_orders_sections() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &set_with_demo(dir.path()),
            Some("# Project instructions (repo/AGENTS.md)\n\nrule"),
            "2025-01-15",
            None,
        );
        let base = prompt.find(super::PRINCIPLES).unwrap();
        let env = prompt.find("# Environment").unwrap();
        let skills = prompt.find("# Skills").unwrap();
        let instructions = prompt.find("# Project instructions").unwrap();
        assert!(base < env);
        assert!(env < skills);
        assert!(skills < instructions);
    }

    #[test]
    fn system_prompt_carries_language_policy() {
        let prompt = super::build_system_prompt(
            Path::new("/work"),
            &SkillSet::default(),
            None,
            "2025-01-15",
            None,
        );
        assert!(prompt.contains("Reply to the user in their language"));
        assert!(prompt.contains("keep code, identifiers, paths, commands, tool arguments"));
        assert!(prompt.contains("project's prevailing language"));
    }

    #[test]
    fn language_anchor_appends_only_for_top_run() {
        use goat_provider::ContentBlock;
        let base = vec![ContentBlock::text_result("call_1", "ok")];
        let top = super::append_language_anchor(base.clone(), true);
        assert_eq!(top.len(), 2);
        assert!(matches!(
            top.last(),
            Some(ContentBlock::Text { text }) if text == super::LANGUAGE_REMINDER
        ));
        let child = super::append_language_anchor(base, false);
        assert_eq!(child.len(), 1);
        assert!(matches!(
            child.last(),
            Some(ContentBlock::ToolResult { .. })
        ));
    }
}
