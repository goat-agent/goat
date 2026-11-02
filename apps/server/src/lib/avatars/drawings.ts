/** Which drawing of each part makes an agent's default picture. */
export type AvatarParts = { background: number; horns: number; eyes: number };

// System colors, without purple and indigo, which mean "the user" in the apps.
const backgrounds = ["#ff9f0a", "#30d158", "#0a84ff", "#ff375f", "#64d2ff", "#ffd60a", "#ac8e68"];

const horns = [
  "M30 30q-6-14 4-20q2 10 8 16zM70 30q6-14-4-20q-2 10-8 16z",
  "M27 33q-12-17-1-29q3 14 11 23zM73 33q12-17 1-29q-3 14-11 23z",
];

const eyes = [
  '<ellipse cx="38" cy="54" rx="5" ry="7"/><ellipse cx="62" cy="54" rx="5" ry="7"/>',
  '<circle cx="38" cy="55" r="5.5"/><circle cx="62" cy="55" r="5.5"/>',
  '<path d="M32 56q6-7 12 0M56 56q6-7 12 0" fill="none" stroke="#1d1d1f" stroke-width="5" stroke-linecap="round"/>',
];

export const partCounts = {
  background: backgrounds.length,
  horns: horns.length,
  eyes: eyes.length,
} satisfies Record<keyof AvatarParts, number>;

// Apps cache a picture forever by its id, so changing a drawing needs a new revision.
const revision = 1;

export function defaultAvatarId(parts: AvatarParts): string {
  return `default-${revision}-${parts.background}-${parts.horns}-${parts.eyes}`;
}

export function drawAvatar(parts: AvatarParts): string {
  return [
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">',
    `<circle cx="50" cy="50" r="50" fill="${backgrounds[parts.background]}"/>`,
    `<path d="${horns[parts.horns]}" fill="#000" fill-opacity="0.18"/>`,
    `<g fill="#1d1d1f">${eyes[parts.eyes]}</g>`,
    "</svg>",
  ].join("");
}
