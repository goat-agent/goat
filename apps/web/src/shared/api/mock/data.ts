import {
  type Agent,
  apiBase,
  type Device,
  type Integration,
  type Message,
  type User,
} from "../client";

// One account that shows every state the screens have: a working agent, a reply that asks to
// connect a service, an account that needs reconnecting, steps taken on a Mac, files, and an
// agent with no picture and no messages.

const minutesAgo = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

export const me: User = {
  id: "user",
  email: "mino@example.com",
  name: "민오",
  timeZone: "Asia/Seoul",
};

export const agents: Agent[] = [
  {
    id: "0198a000-0000-7000-8000-000000000001",
    name: "미나",
    instructions:
      "일정과 회의를 챙겨 줘. 회의록은 제품팀 노션에 올리고, 맥 캘린더를 써도 돼. 답은 짧게.",
    avatarId: "0198a000-0000-7000-8000-0000000000a1",
    status: "running",
    createdAt: minutesAgo(60 * 24 * 30),
  },
  {
    id: "0198a000-0000-7000-8000-000000000002",
    name: "준",
    instructions: "",
    avatarId: "0198a000-0000-7000-8000-0000000000a2",
    status: "idle",
    createdAt: minutesAgo(60 * 24 * 20),
  },
  {
    id: "0198a000-0000-7000-8000-000000000003",
    name: "아틀라스",
    instructions: "",
    avatarId: "0198a000-0000-7000-8000-0000000000a3",
    status: "idle",
    createdAt: minutesAgo(60 * 24 * 10),
  },
  {
    id: "0198a000-0000-7000-8000-000000000004",
    name: "가계부",
    instructions: "",
    status: "idle",
    createdAt: minutesAgo(60 * 24 * 7),
  },
];

const [mina, jun, atlas] = agents.map(({ id }) => id);

export const conversations = new Map<string, Message[]>([
  [
    mina ?? "",
    [
      {
        id: 1,
        role: "user",
        text: "내일 오전 회의 내용 정리해서 팀 노션에 올려줘",
        createdAt: minutesAgo(60 * 26),
      },
      {
        id: 2,
        role: "assistant",
        text: "노션에 올리려면 먼저 연결해 주세요. 연결하면 바로 이어서 할게요.",
        connect: [{ integrationId: "notion", name: "Notion" }],
        createdAt: minutesAgo(60 * 26 - 1),
      },
      {
        id: 3,
        role: "assistant",
        text: "올렸어요. 정리하면 이래요.\n\n- 출시일은 11월 14일\n- 랜딩 문구는 준, QA 일정은 미나\n- 가격 페이지는 다음 회의에서",
        createdAt: minutesAgo(60 * 25),
      },
      {
        id: 4,
        role: "user",
        text: "이 일정 맥 캘린더에 넣어줘",
        files: [
          {
            id: "0198a000-0000-7000-8000-0000000000f1",
            name: "screenshot.png",
            mediaType: "image/png",
          },
        ],
        createdAt: minutesAgo(40),
      },
      {
        id: 5,
        role: "assistant",
        text: "넣었어요. 11월 3일 월요일 오전 10시부터 한 시간이에요.",
        steps: ["캘린더 열기", "11월 3일 칸 누르기", "‘디자인 리뷰’ 입력하고 오전 10시로 저장"],
        createdAt: minutesAgo(38),
      },
      {
        id: 6,
        role: "user",
        text: "이것도 세 줄로 요약해줘",
        files: [
          {
            id: "0198a000-0000-7000-8000-0000000000f2",
            name: "4분기-계획.pdf",
            mediaType: "application/pdf",
          },
        ],
        createdAt: minutesAgo(1),
      },
    ],
  ],
  [
    jun ?? "",
    [
      { id: 11, role: "user", text: "열린 PR 리뷰해줘", createdAt: minutesAgo(200) },
      {
        id: 12,
        role: "assistant",
        text: "PR 세 개 리뷰 남겼어요. 하나는 테스트가 빠졌어요.",
        createdAt: minutesAgo(180),
      },
    ],
  ],
  [
    atlas ?? "",
    [
      { id: 21, role: "user", text: "도쿄 항공권 최저가 알려줘", createdAt: minutesAgo(60 * 30) },
      {
        id: 22,
        role: "assistant",
        text: "메일에서 예약 내역을 보려면 Google을 다시 연결해 주세요.",
        connect: [{ integrationId: "google", name: "Google" }],
        createdAt: minutesAgo(60 * 29),
      },
    ],
  ],
]);

export const integrations: Integration[] = [
  {
    id: "notion",
    name: "Notion",
    iconUrl: `${apiBase}/integrations/notion/icon`,
    connections: [
      {
        id: "0198a000-0000-7000-8000-0000000000c1",
        accountName: "제품팀 워크스페이스",
        needsReconnect: false,
      },
    ],
  },
  {
    id: "google",
    name: "Google",
    iconUrl: `${apiBase}/integrations/google/icon`,
    connections: [
      {
        id: "0198a000-0000-7000-8000-0000000000c2",
        accountName: "mino@gmail.com",
        needsReconnect: true,
      },
    ],
  },
  { id: "github", name: "GitHub", iconUrl: `${apiBase}/integrations/github/icon`, connections: [] },
  { id: "linear", name: "Linear", iconUrl: `${apiBase}/integrations/linear/icon`, connections: [] },
];

export const devices: Device[] = [
  {
    id: "0198a000-0000-7000-8000-0000000000d1",
    name: "Chrome · Mac",
    platform: "web",
    isCurrent: true,
    createdAt: minutesAgo(10),
  },
  {
    id: "0198a000-0000-7000-8000-0000000000d2",
    name: "민오의 iPhone",
    platform: "ios",
    isCurrent: false,
    createdAt: minutesAgo(60 * 24 * 3),
  },
  {
    id: "0198a000-0000-7000-8000-0000000000d3",
    name: "MacBook Pro",
    platform: "macos",
    isCurrent: false,
    createdAt: minutesAgo(60 * 24 * 9),
  },
];

const faces = ["#ffb3c7,#c9a7ff", "#9fe3c9,#6cb8ff", "#ffd59e,#ff9d7a", "#d6e4ff,#a3b8ff"];

/** A gradient face standing in for an uploaded picture. */
export function faceSvg(seed: string): string {
  const index = Array.from(seed).reduce((sum, char) => sum + char.charCodeAt(0), 0) % faces.length;
  const [from, to] = (faces[index] ?? "").split(",");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${from}"/><stop offset="1" stop-color="${to}"/></linearGradient></defs><rect width="64" height="64" fill="url(#g)"/><circle cx="24" cy="29" r="3.2" fill="#2b2b2b"/><circle cx="40" cy="29" r="3.2" fill="#2b2b2b"/><path d="M25 39c4 4 10 4 14 0" stroke="#2b2b2b" stroke-width="3" fill="none" stroke-linecap="round"/></svg>`;
}

export function iconSvg(letter: string): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48"><rect width="48" height="48" fill="#fff"/><text x="24" y="31" font-family="system-ui" font-size="22" font-weight="700" text-anchor="middle" fill="#111">${letter}</text></svg>`;
}

export const screenshotSvg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 240 160"><rect width="240" height="160" fill="#fff"/><rect width="240" height="28" fill="#f4f4f5"/><g fill="#e4e4e7"><rect x="14" y="44" width="212" height="11" rx="4"/><rect x="14" y="64" width="212" height="11" rx="4"/><rect x="14" y="84" width="212" height="11" rx="4" fill="#dbe6ff"/><rect x="14" y="104" width="212" height="11" rx="4"/></g></svg>`;
