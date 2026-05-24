# Hermes Agent 공식 출처 리서치 근거

- 기준일: 2026-05-19 KST
- 범위: Hermes Agent 공식 GitHub, 공식 문서, 릴리스, 보안 정책 중심
- 범위 제한: 본 문서는 Hermes Agent 공식 근거를 정리한다.

## 1. 핵심 결론
Hermes Agent는 Nous Research가 만든 MIT 라이선스의 Python 기반 self-improving 개인 AI agent다. 공식 문서는 Hermes Agent를 터미널, 게이트웨이, IDE/ACP, API/배치 진입점에서 동작하는 자율 에이전트로 설명하며, persistent memory, agent-created skills, multi-platform messaging gateway, scheduler, delegation/subagents, tool calling, MCP support, multiple execution backends를 핵심 기능으로 제시한다. 2026-05-19 KST 확인 기준 최신 공식 GitHub release는 **Hermes Agent v0.14.0 / tag `v2026.5.16`, 2026-05-16 release**다.

## 2. 공식 출처 및 근거
- GitHub repo: https://github.com/NousResearch/hermes-agent
  - Public repo under `NousResearch/hermes-agent`, MIT license, Python primary language, homepage `https://hermes-agent.nousresearch.com`.
  - GitHub API snapshot retrieved 2026-05-19 KST: created `2025-07-22T22:22:28Z`, pushed `2026-05-18T16:40:46Z`, stars `156122`, forks `25087`, open issues `12094`, default branch `main`, license MIT.
- Docs hub: https://hermes-agent.nousresearch.com/docs/
  - Describes Hermes as a self-improving AI agent with built-in learning loop, skill creation/improvement, persistent knowledge, session search, and user modeling.
  - Quick install paths include Linux/macOS/WSL2, native Windows early beta, and Android/Termux.
  - Key features include 6+ terminal backends, 20+ messaging platforms, cron, delegation, skills, web control, MCP, and research/training trajectory support.
- Installation docs: https://hermes-agent.nousresearch.com/docs/getting-started/installation
  - One-line Linux/macOS/WSL2 installer tracks `main`.
  - Native Windows is marked early beta; WSL2 remains the most battle-tested Windows route.
  - Windows installer handles uv, Python 3.11, Node.js 22, ripgrep, ffmpeg, PortableGit/Git Bash, repo clone, venv, PATH shim, and setup.
  - Native Windows feature parity excludes the dashboard `/chat` embedded terminal pane, which needs POSIX PTY / WSL2.
- Windows native guide: https://hermes-agent.nousresearch.com/docs/user-guide/windows-native
  - Confirms native Windows 10/11 support is early beta and works without WSL/Cygwin/Docker for most features.
  - Shell commands run through Git Bash; data layout splits disposable code/deps under `%LOCALAPPDATA%\hermes` and user data under `%USERPROFILE%\.hermes`.
- Architecture docs: https://hermes-agent.nousresearch.com/docs/developer-guide/architecture
  - Entry points: CLI, gateway, ACP, batch runner, API server, Python library.
  - Core `AIAgent` in `run_agent.py`; subsystems include prompt builder, provider resolution, tool dispatch, SQLite+FTS5 session storage, terminal/browser/web/MCP/file/vision tool backends.
  - Directory map lists `agent/`, `gateway/`, `tools/`, `hermes_cli/`, `plugins/`, `skills/`, `optional-skills/`, `tests/`.
  - Major subsystems: agent loop, prompt system, provider resolution, tool registry, session persistence, messaging gateway, plugin system, cron, ACP, trajectory generation.
- Tools docs: https://hermes-agent.nousresearch.com/docs/user-guide/features/tools
  - Tools are grouped into configurable toolsets per platform.
  - Built-in categories cover web, X search, terminal/files, browser, media, orchestration, memory/recall, automation/delivery, Home Assistant/MCP/RL.
  - Terminal backends include local, Docker, SSH, Singularity, Modal, Daytona, and Vercel Sandbox.
  - Container backends use hardening such as read-only root FS for Docker, dropped Linux capabilities, no privilege escalation, PID limits, namespace isolation, and persistent workspace volumes.
- Configuration docs: https://hermes-agent.nousresearch.com/docs/user-guide/configuration/
  - `~/.hermes/` contains `config.yaml`, `.env`, `auth.json`, `SOUL.md`, `memories/`, `skills/`, `cron/`, `sessions/`, and `logs/`.
  - Configuration precedence is CLI args, `~/.hermes/config.yaml`, `~/.hermes/.env`, then built-in defaults; secrets belong in `.env`.
- Memory docs: https://hermes-agent.nousresearch.com/docs/user-guide/features/memory
  - Memory is injected at session start; live memory changes persist to disk immediately but do not alter the current system prompt until the next session.
  - Two targets: `memory` for environment/workflow/project facts and `user` for user identity/preferences.
  - Memory tool supports `add`, `replace`, and `remove`; no `read` action because memory is automatically injected.
- Skills docs: https://hermes-agent.nousresearch.com/docs/user-guide/features/skills
  - Installed skills are slash commands usable from CLI or messaging platforms.
  - Skills use progressive disclosure: list metadata first, then full `SKILL.md`, then specific reference files.
  - `SKILL.md` supports metadata such as platform restrictions, required/fallback toolsets, and config hints.
- Messaging docs: https://hermes-agent.nousresearch.com/docs/user-guide/messaging
  - Gateway is a single background process for Telegram, Discord, Slack, WhatsApp, Signal, SMS, Email, Home Assistant, Mattermost, Matrix, DingTalk, Feishu/Lark, WeCom, Weixin, BlueBubbles/iMessage, QQ, Yuanbao, Microsoft Teams, LINE, browser/webhooks, etc.
  - Gateway routes platform messages through per-chat session storage to `AIAgent`, runs cron every 60 seconds, and supports slash commands such as `/new`, `/model`, `/status`, `/approve`, `/deny`, `/stop`, `/compress`, `/resume`, `/voice`.
- Subagent delegation docs: https://hermes-agent.nousresearch.com/docs/user-guide/features/delegation
  - `delegate_task` spawns child `AIAgent` instances with fresh isolated context, restricted toolsets, and separate terminal sessions; only the final summary enters the parent context.
  - Batch delegation runs up to 3 concurrent subagents by default, configurable via `delegation.max_concurrent_children` or `DELEGATION_MAX_CONCURRENT_CHILDREN`.
  - Leaf children cannot use delegation, clarify, memory, code execution, or send_message toolsets by default; nested orchestration is opt-in.
- Release v0.14.0: https://github.com/NousResearch/hermes-agent/releases/tag/v2026.5.16
  - Latest official release observed: **Hermes Agent v0.14.0 (v2026.5.16)**, released May 16, 2026.
  - Release stats since v0.13.0: 808 commits, 633 merged PRs, 1,393 files changed, 165,061 insertions, 545 issues closed, 215 community contributors.
  - Highlights: xAI Grok via SuperGrok OAuth; OpenAI-compatible local proxy for OAuth-backed providers; first-class `x_search`; Microsoft Teams end-to-end; lighter/lazy installs and supply-chain advisory checker; PyPI `pip install hermes-agent`; cross-session 1h Claude prompt cache; 180x faster browser console; LINE + SimpleX Chat; `/handoff`; LSP diagnostics on writes; unified `video_generate`; non-Anthropic `computer_use` via cua-driver; Zed ACP registry; native Windows early beta.
- Security docs: https://hermes-agent.nousresearch.com/docs/user-guide/security
  - User-facing security page describes seven defense-in-depth layers: user authorization, dangerous command approval, container isolation, MCP credential filtering, context file scanning, cross-session isolation, input sanitization.
  - Approval modes: `manual` default, `smart`, and `off`; YOLO mode bypasses prompts but hardline blocklist remains always-on.
  - Gateway authorization uses platform/global allowlists and DM pairing; default is deny if no allowlist/allow-all is configured.
  - Docker backend hardening: capability drop, no-new-privileges, PID/tmpfs limits, persistent/ephemeral filesystem modes.
  - Production checklist: explicit allowlists, container backend, resource limits, secure secrets, DM pairing, review allowlists, set messaging CWD.
- SECURITY.md: https://github.com/NousResearch/hermes-agent/blob/main/SECURITY.md
  - Security reports go through GitHub Security Advisories or `security@nousresearch.com`; no public issue for vulnerabilities.
  - Trust model: single-tenant personal agent; the real adversarial boundary is OS-level isolation / whole-process wrapping, not in-process heuristics.
  - Whole-process wrapping is the supported posture for untrusted input surfaces or production/shared deployments.
  - Credential filtering reduces casual leakage but is not containment; skills/plugins/hooks inside the agent process can access what the agent can access.
  - In-process heuristics such as approval gate, output redaction, and Skills Guard are useful but explicitly not security boundaries.
  - External surfaces require authorization at trust-boundary crossings; network-exposed adapters require allowlists and must not fail open.
  - Out of scope as vulnerabilities: prompt injection alone without a chained boundary outcome, bypassing in-process heuristics, local-backend host access, public exposure without external controls, third-party malicious skills/plugins as operator review surface.

## 3. 버전 및 변동성 메모
- Evidence checked against official docs/GitHub pages on 2026-05-19 KST.
- The project is very active: GitHub repo API showed `pushed_at` 2026-05-18T16:40:46Z and the v0.14.0 release page said 272 commits to `main` since the May 16 release. Treat exact repo metrics as timestamped, not stable.
- Docs and README have small count discrepancies (for example docs hub says 6 terminal backends while README/release/tool docs mention newer additions such as Vercel Sandbox and 7 backends). Prefer page-specific current docs for detailed claims and flag counts as moving targets.

## 4. 불확실성 / 문서화 시 주의
- Official user-facing security docs emphasize defense-in-depth; SECURITY.md narrows what counts as a real security boundary. Final Korean document should present both: "many safety layers exist" but "only OS-level / whole-process isolation should be treated as adversarial containment."
- Native Windows support is real but explicitly early beta; WSL2 remains the safer recommendation for production-like Windows use, especially dashboard embedded terminal behavior.
- Release notes are extensive and fast-moving; final document should cite the v0.14.0 release directly for May 16 highlights rather than relying on secondary summaries.
- Third-party posts, mirrors, Reddit summaries, and generated docs PDFs were observed in search results but were not used as authoritative evidence for this document.

## 5. 한국어 정리용 핵심 bullet
- **정체**: Hermes Agent는 Nous Research가 만든 MIT 라이선스 오픈소스 개인용/self-improving AI agent로, CLI·메시징 게이트웨이·IDE/ACP·API/배치 진입점을 통해 동작한다.
- **핵심 차별점**: 기억(`MEMORY.md`/사용자 프로필), 세션 검색(SQLite+FTS5), 자체 스킬 생성/개선, 백그라운드 review/curator 루프를 통해 사용 경험이 누적될수록 재사용 가능한 절차 지식이 늘어나는 구조를 강조한다.
- **운영 위치**: 로컬 노트북에 묶인 도구라기보다 VPS, GPU 서버, Docker/SSH/Modal/Daytona/Vercel Sandbox 등 여러 실행 백엔드에서 돌리고 Telegram/Discord/Slack/WhatsApp/Signal/Teams/LINE 등에서 호출하는 “상주형 개인 에이전트”에 가깝다.
- **상태/설정 디렉터리**: 기본 상태는 `~/.hermes/` 아래 `config.yaml`, `.env`, `auth.json`, `SOUL.md`, `memories/`, `skills/`, `cron/`, `sessions/`, `logs/`로 분리된다. 비밀값은 `.env`, 일반 설정은 `config.yaml`에 두는 모델이다.
- **아키텍처**: `AIAgent`가 prompt assembly, provider resolution, tool dispatch, compression/caching, persistence를 담당하고, gateway/cron/ACP/API/batch가 같은 코어를 호출한다.
- **도구 체계**: web/search, terminal/file, browser, media, delegation, memory/session_search, cron/send_message, MCP/RL/Home Assistant 등이 toolset으로 묶이며 플랫폼별로 활성화 가능하다.
- **위임/병렬화**: `delegate_task`는 fresh context의 child `AIAgent`를 만들고 최종 요약만 parent context로 돌려준다. 기본 병렬 child 수는 3이며 leaf child는 재귀 위임·clarify·memory·send_message·execute_code가 제한된다.
- **설치/배포**: Linux/macOS/WSL2는 one-line installer, Windows native는 PowerShell installer early beta, PyPI `pip install hermes-agent`는 v0.14.0에서 공식 강조점이 되었다.
- **보안 모델**: 기본 local backend는 호스트에서 명령을 실행하므로 신뢰된 개인 운영자 전제가 강하다. untrusted input/공개 gateway/프로덕션은 Docker/Modal/Daytona/Vercel/OpenShell 같은 전체 프로세스 격리를 전제로 설명해야 한다.
- **승인/가드레일**: dangerous command approval, hardline blocklist, allowlist/DM pairing, MCP env filtering, SSRF/private URL 차단, context injection scanning, Tirith scanning 등이 있지만 SECURITY.md는 이를 “보조 휴리스틱”으로 보고 실질 경계는 OS-level isolation이라고 못박는다.
- **최신 v0.14.0 포인트**: Foundation Release는 설치/배포 경량화, PyPI 배포, native Windows beta, OpenAI-compatible local proxy, xAI Grok/SuperGrok OAuth, X search, Teams/LINE/SimpleX 확장, browser/CDP 성능 개선, LSP diagnostics, `computer_use` 확장 등을 포함한다.
- **불확실성 표기**: 스타/포크/이슈 수와 플랫폼/백엔드 개수는 빠르게 변한다. 문서에는 “2026-05-19 KST 확인치”로만 적고 고정 사실처럼 쓰지 않는다.

## 6. 검증
- PASS: Official GitHub repository, GitHub API, official docs hub, installation, architecture, tools, memory, skills, messaging, security docs, SECURITY.md, and v0.14.0 release page inspected.
- PASS: Evidence separated into official docs/release/security; third-party search results were not used as authoritative facts.
- N/A: Typecheck/test/lint are not applicable to this research-only Markdown evidence report; no production code was modified.

## 7. 작업 근거
- Subagents spawned: 3 (`019e3d84-9564-7443-822e-c2edfe10d039` docs/features probe, `019e3d84-95f6-7453-b836-5302439f27e9` releases/changelog probe, `019e3d84-965f-73a3-91e4-edc109d81d4d` security/trust-model probe).
- Subagent model requested: `gpt-5.4-mini`.
- Serial searches before spawn: 0 post-claim research searches; spawned immediately after claim/context load.
- Findings integrated: child probes did not return before two waits; direct official-source verification above was used to avoid blocking the team.
- Late child finding integrated: docs/features probe `019e3d84-9564-7443-822e-c2edfe10d039` returned; added configuration directory/precedence and delegation behavior details.
