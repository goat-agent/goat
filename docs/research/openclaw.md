# OpenClaw 공식 출처 리서치 근거

- 기준일: 2026-05-19 KST
- 범위: OpenClaw 공식 GitHub, 공식 문서, 릴리스, 보안 정책/Advisory 중심
- 범위 제한: 본 문서는 OpenClaw 공식 근거를 정리한다.

## 1. 핵심 결론

OpenClaw는 사용자가 직접 운영하는 개인용/local-first AI assistant Gateway다. 하나의 장기 실행 Gateway가 메신저 채널, 세션, 라우팅, WebChat/Control UI, 노드 연결, 도구 실행 및 운영 명령의 중심이 된다. 공식 문서와 README는 OpenClaw를 “호스팅 서비스”가 아니라 사용자의 기기 또는 서버에서 구동하는 개인 비서형 게이트웨이로 설명한다.

보안 모델은 “신뢰된 단일 운영자” 전제가 핵심이다. 하나의 Gateway를 적대적 다중 사용자 간의 강한 격리 경계로 보지 않으며, 혼합 신뢰 환경에서는 별도 Gateway/OS 사용자/호스트/VPS 등으로 경계를 분리해야 한다. 기본 main 세션의 도구는 호스트에서 실행될 수 있으므로, 그룹/공유/비-main 세션에서는 sandbox와 tool policy를 명시적으로 구성하는 것이 중요하다.

## 2. 공식 출처 및 근거

### 공식 문서

- OpenClaw Docs Home: https://docs.openclaw.ai/
  - 제품 정체, Gateway 개념, 주요 채널, self-hosted / multi-channel / agent-native / MIT 오픈소스 포지션 확인.
- Getting Started: https://docs.openclaw.ai/start/getting-started
  - 설치, onboarding, Gateway 상태 확인, dashboard 실행, 첫 메시지 흐름 확인.
- Install: https://docs.openclaw.ai/install
  - installer script, npm/pnpm/bun, source, Docker/Podman/Nix 등 설치 경로와 검증 명령 확인.
- Gateway Architecture: https://docs.openclaw.ai/concepts/architecture
  - 장기 실행 Gateway, WebSocket 클라이언트/노드, protocol, pairing, remote access, 운영 invariant 확인.
- Multi-Agent Routing: https://docs.openclaw.ai/concepts/multi-agent
  - agent별 workspace, `agentDir`, auth profile, session store, binding 기반 라우팅, sandbox/tool config 확인.
- Chat Channels: https://docs.openclaw.ai/channels
  - 지원 채널 목록, 동시 실행, DM pairing/allowlist 안전장치 확인.
- Tools and Plugins: https://docs.openclaw.ai/tools
  - built-in tools, plugin-provided tools, allow/deny list, tool profile/group 확인.
- Sandboxing: https://docs.openclaw.ai/gateway/sandboxing
  - Docker/SSH/OpenShell backend, workspace access, bind mount, network default, browser sandbox 제약 확인.

### 공식 GitHub / 릴리스 / 보안

- Repository: https://github.com/openclaw/openclaw
  - 공식 본체 저장소, MIT license, README, stars/forks/issues 등 GitHub 공개 메타데이터 확인.
- Raw README: https://raw.githubusercontent.com/openclaw/openclaw/main/README.md
  - 개인 assistant 정체, 지원 채널, 설치/quick start, security defaults, highlights 확인.
- Latest release: https://github.com/openclaw/openclaw/releases/latest
  - 2026-05-19 기준 `v2026.5.18` latest release 확인.
- Releases list: https://github.com/openclaw/openclaw/releases
  - 릴리스 흐름 및 최근 변경 요약 확인.
- Security policy: https://github.com/openclaw/openclaw/blob/main/SECURITY.md
  - private disclosure, trust model, out-of-scope, operator/plugin/model assumptions 확인.
- Raw SECURITY.md: https://raw.githubusercontent.com/openclaw/openclaw/main/SECURITY.md
  - HTML 렌더링 오류 대비 원문 정책 검증.
- Security Advisories: https://github.com/openclaw/openclaw/security/advisories
  - 공개 GHSA 목록과 최근 보안 이슈 존재 확인.
- Advisory example — Browser SSRF Policy Bypass: https://github.com/openclaw/openclaw/security/advisories/GHSA-vr5g-mmx7-h897
  - affected `<=2026.4.5`, patched `2026.4.8`, Moderate 사례.
- Advisory example — Plugin install path traversal: https://github.com/openclaw/openclaw/security/advisories/GHSA-qrq5-wjgg-rvqw
  - affected `>=2026.1.29-beta.1, <2026.2.1`, patched `>=2026.2.1`, Critical 사례.

## 3. 확인된 핵심 사실

### 3.1 정체와 목적

- OpenClaw는 “개인용 AI assistant”로 포지셔닝되어 있으며, 사용자가 자신의 기기 또는 서버에서 실행한다.
- Gateway는 제품의 제어 평면(control plane)이다. 채팅 앱, channel plugin, WebChat, mobile node를 하나의 Gateway 프로세스에 연결한다.
- 대상 사용자는 개발자와 power user다. 공식 문서는 “자기 데이터 통제”와 “always-on 개인 assistant”를 강조한다.

### 3.2 아키텍처

- 공식 아키텍처 문서 기준, 하나의 장기 실행 Gateway가 메시징 표면과 provider/channel connection을 소유한다.
- Control-plane clients(macOS app, CLI, web UI, automation)와 nodes(macOS/iOS/Android/headless)는 Gateway WebSocket API로 연결된다.
- 기본 Gateway 포트는 `127.0.0.1:18789`로 문서화되어 있다.
- Gateway는 request/response/event 형태의 typed WebSocket API를 제공하고 JSON Schema로 inbound frame을 검증한다.
- pairing은 device identity 기반이며, local loopback은 UX를 위해 auto-approve 될 수 있으나 LAN/tailnet/remote 연결은 명시적 pairing approval이 필요하다.

### 3.3 세션, 메모리, 멀티 에이전트

- multi-agent routing의 단위인 agent는 자체 workspace, state directory(`agentDir`), auth profile, model registry/config, session store를 가진다.
- 세션 저장 위치는 공식 문서상 `~/.openclaw/agents/<agentId>/sessions` 계열로 설명된다.
- 단일 agent 기본값은 `main`이며, workspace는 기본적으로 `~/.openclaw/workspace` 계열이다.
- binding은 inbound message를 agent로 결정한다. 문서는 peer, parent/thread, Discord guild/roles, Slack team, account, channel, default agent 순의 라우팅 우선순위를 설명한다.
- multi-agent는 workspace와 세션/상태 분리 구조를 제공하지만, workspace 자체가 hard sandbox는 아니다. sandboxing을 켜지 않으면 absolute path로 host의 다른 위치에 접근할 수 있다는 문서상 caveat가 있다.

### 3.4 채널과 도구

- 공식 채널 문서와 README 기준 주요 채널은 WhatsApp, Telegram, Slack, Discord, Google Chat, Signal, iMessage/BlueBubbles, IRC, Microsoft Teams, Matrix, Feishu, LINE, Mattermost, Nextcloud Talk, Nostr, QQ, Twitch, Zalo, WebChat 등이다.
- 채널은 동시에 실행될 수 있고 Gateway가 chat 단위로 라우팅한다.
- built-in tools에는 exec/process, code execution, browser, web search/fetch, file read/write/edit, apply_patch, message, canvas, nodes, cron/gateway, image/music/video generation, TTS, sessions/subagents 등이 포함된다.
- tools.allow / tools.deny, tools.profile, group:* shorthand로 도구 노출 범위를 제어한다. deny가 allow보다 우선한다.
- plugin은 channels, model providers, tools, skills, speech/realtime transcription/voice, media understanding, image/video/music, web fetch/search 등을 확장할 수 있다.

### 3.5 설치와 운영

- 공식 설치 문서의 recommended path는 installer script이며, Node를 감지/설치하고 OpenClaw 설치 후 onboarding을 실행한다.
- npm/pnpm/bun global install도 제공되며, Gateway daemon 설치는 `openclaw onboard --install-daemon` 흐름으로 안내된다.
- 설치 검증 명령은 `openclaw --version`, `openclaw doctor`, `openclaw gateway status`다.
- 공식 README와 최신 릴리스는 Node 24를 추천한다. Node 22 계열에 대한 최소 patch level 설명은 문서/README/release 사이에 차이가 있으므로 “Node 24 권장, Node 22는 최신 지원 patch 사용”으로 쓰는 것이 안전하다.
- 최신 릴리스 `v2026.5.18`은 2026-05-18 게시된 GitHub latest release로 확인된다. 주요 변경에는 Pi package 업데이트, Node 22.19+ 최소선 상향, Gateway startup/restart readiness trace 개선, plugin SDK/build/validate/init, proxy TLS CA 지원, QA runtime parity/tool coverage 강화, Android realtime Talk Mode, media/Gateway/Codex 관련 안정화가 포함된다.

## 4. 보안 모델과 운영상 주의점

- OpenClaw SECURITY.md는 OpenClaw를 local-first trusted-operator infrastructure로 규정한다.
- 하나의 Gateway는 적대적 다중 사용자 보안 경계가 아니다. 여러 신뢰 경계가 섞이면 separate gateway, separate credentials, 가능하면 separate OS user/host/VPS를 사용해야 한다.
- prompt injection만으로는 일반적으로 보안 취약점으로 보지 않는다. 보안 보고는 auth, approval, sandbox, policy, tool boundary 등 문서화된 경계를 실제로 우회했음을 보여야 한다.
- authenticated Gateway caller는 해당 Gateway의 trusted operator로 취급된다. sessionKey/session label은 라우팅 컨트롤이지 per-user authorization boundary가 아니다.
- 기본 exec behavior는 host-first로 설명된다. `agents.defaults.sandbox.mode` 기본값은 off이며, isolation이 필요하면 `non-main` 또는 `all` sandbox mode와 strict tool policy를 적용해야 한다.
- plugin/extension은 Gateway와 같은 process 권한으로 로드되는 trusted code다. 따라서 신뢰 가능한 plugin만 설치하고 `plugins.allow` 등 allowlist를 사용하는 것이 권장된다.
- sandbox backend는 Docker(default), SSH, OpenShell이 있다. Docker sandbox는 기본 network가 none이고 browser sandbox를 지원하지만, SSH/OpenShell backend는 browser sandbox 지원에 제약이 있다.
- Docker bind mount는 sandbox filesystem boundary를 우회해 host path를 노출할 수 있으므로, 문서는 credential roots나 dangerous paths 차단 및 read-only mount 권장을 명시한다.

## 5. 공개 보안 Advisory 관찰

공식 GitHub Security Advisories에는 다수의 공개 GHSA가 존재한다. 이는 프로젝트가 취약점 보고/패치 프로세스를 운영하고 있다는 근거인 동시에, agentic local assistant 특성상 plugin, browser, gateway auth, sandbox bridge, owner command 등 보안 경계가 실제 공격/검토 대상임을 보여준다.

대표 사례:

- `GHSA-vr5g-mmx7-h897` Browser SSRF Policy Bypass via Interaction-Triggered Navigation
  - 심각도: Moderate
  - 영향 버전: `<= 2026.4.5`
  - 패치 버전: `2026.4.8`
  - 요지: browser interaction이 정상 SSRF navigation check를 우회할 수 있었던 사례.
- `GHSA-qrq5-wjgg-rvqw` Path Traversal in Plugin Installation
  - 심각도: Critical
  - 영향 버전: `>= 2026.1.29-beta.1, < 2026.2.1`
  - 패치 버전: `>= 2026.2.1`
  - 요지: malicious plugin package name이 extensions directory 밖으로 path traversal write를 유발할 수 있었던 사례.

## 6. 불확실성 / 문서화 시 주의

- GitHub stars/forks/issues/security count는 live HTML에서 관찰되는 값이라 기준일 이후 변동된다. 2026-05-19 기준 대략 stars 373k, forks 77k 수준으로만 표현하고, 정밀 수치처럼 고정하지 않는 것이 안전하다.
- Node 22 최소 버전은 공식 문서/README/릴리스 간 문구가 다르다. 최종 문서에는 “Node 24 권장, Node 22는 최신 지원 patch 사용”으로 쓰고, 특정 patch level은 출처별 차이를 각주로 남기는 것이 좋다.
- built-in / bundled plugin / external plugin 구분은 릴리스와 설치 상태에 따라 달라질 수 있으므로, 채널/기능 목록은 “공식 문서가 지원 또는 plugin 기반 지원으로 열거”한다고 표현한다.
- multi-agent isolation은 agent별 상태/세션/workspace 분리이지 host-level sandbox가 아니다. “격리”라는 단어를 쓸 때 sandbox/tool policy 미설정 시 한계를 반드시 붙여야 한다.
- 공식 SECURITY.md는 prompt injection-only report를 보안 취약점 범위 밖으로 두지만, 실제 운영 위험이 없다는 뜻은 아니다. 최종 문서에서는 “취약점 triage 기준”과 “운영 위험”을 분리해 설명해야 한다.
- web search에는 `microsoft/openclaw`, OpenClaw mirror/cheatsheet/SEO PDF/비공식 security guide 등 혼동 소지가 있는 결과가 많다. 본 문서에서는 `openclaw/openclaw`, `docs.openclaw.ai`, `raw.githubusercontent.com/openclaw/openclaw`, `github.com/openclaw/openclaw/security/advisories`만 공식 근거로 사용했다.

## 7. 한국어 정리용 핵심 bullet

- OpenClaw는 사용자가 직접 운영하는 local-first 개인 AI assistant Gateway이며, 여러 메신저 채널을 하나의 장기 실행 Gateway로 연결해 AI agent와 대화하게 한다.
- Gateway는 control plane이다. 채널 연결, 세션, 라우팅, WebChat/Control UI, node 연결, 도구 실행과 운영 명령이 Gateway를 중심으로 동작한다.
- 최신 공식 GitHub release는 2026-05-18 게시된 `v2026.5.18`이며, 2026-05-19 기준 GitHub에서 latest로 표시된다.
- 설치는 installer script, npm/pnpm/bun global install, source build, Docker/Podman/Nix 등으로 가능하며, onboarding 후 `openclaw doctor`와 `openclaw gateway status`로 검증한다.
- 주요 지원 채널은 WhatsApp, Telegram, Slack, Discord, Signal, iMessage/BlueBubbles, Matrix, Microsoft Teams, Google Chat, LINE, Zalo, WebChat 등이다.
- multi-agent routing은 agent별 workspace, `agentDir`, auth profile/config, session store를 분리하고 bindings로 inbound message를 특정 agent에 라우팅한다.
- 이 multi-agent 분리는 host-level security sandbox와 다르다. sandboxing을 켜지 않으면 workspace는 기본 cwd일 뿐이며, 절대 경로 접근 같은 host-level 위험은 남는다.
- built-in tool은 shell/process, browser, web search/fetch, file I/O, apply_patch, message, canvas/nodes, cron/gateway, media generation, sessions/subagents 등으로 넓다.
- OpenClaw의 보안 모델은 trusted single-operator personal assistant 모델이다. 하나의 Gateway를 적대적 다중 사용자 격리 경계로 쓰는 것은 공식적으로 권장되지 않는다.
- prompt injection은 운영상 중요한 위험이지만, 공식 취약점 triage에서는 auth/approval/sandbox/tool boundary 우회가 없는 prompt-injection-only chain을 일반적으로 보안 버그로 보지 않는다.
- plugin/extension은 Gateway와 같은 권한의 trusted in-process code다. 신뢰 가능한 plugin만 설치하고 allowlist를 적용해야 한다.
- 공유/그룹/비-main 세션이나 외부 노출 가능성이 있는 운영에서는 `agents.defaults.sandbox.mode`를 `non-main` 또는 `all`로 두고 strict tool policy, DM pairing/allowlist, Gateway auth/bind, Tailscale/SSH tunnel 같은 접근 제어를 함께 사용해야 한다.
- 공식 GHSA에는 browser SSRF policy bypass, plugin install path traversal 등 공개 advisory가 존재한다. 최신 버전 유지와 release/security advisory 모니터링이 필수다.

## 8. 검증 및 작업 근거

- Subagents spawned: 2
  - `019e3d84-b8c9-7693-aae0-45d870a0c92f`: OpenClaw official docs evidence
  - `019e3d84-b931-7fc1-ab3c-9a27238ef1b4`: OpenClaw GitHub/release/security evidence
- Subagent model requested: gpt-5.4-mini
- Findings integrated:
  - docs identity/architecture/channel/tools/install/multi-agent/session/memory bullets
  - GitHub latest release/security policy/advisory/license/repo metadata bullets
- Serial searches before spawn: 0
