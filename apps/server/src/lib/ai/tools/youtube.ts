import { tool } from "ai";
import { z } from "zod";
import { callGoogle, GoogleConnection, googleScopes, untrusted, UNTRUSTED_NOTE } from "./google";
import type { Fetch } from "../../integrations/integration";

const API = "https://www.googleapis.com/youtube/v3";
const metadata = { scope: googleScopes.youtube };
const readMetadata = { ...metadata, effect: "read" };

const DESCRIPTION_CHARS = 2_000;

const Count = z.coerce.number().optional();

const Video = z.object({
  id: z.string(),
  snippet: z.object({
    title: z.string(),
    channelTitle: z.string().default(""),
    channelId: z.string().optional(),
    publishedAt: z.string().optional(),
    description: z.string().default(""),
  }),
  contentDetails: z.object({ duration: z.string().optional() }).default({}),
  statistics: z.object({ viewCount: Count, likeCount: Count, commentCount: Count }).default({}),
});

const Videos = z.object({
  items: z.array(Video).default([]),
  nextPageToken: z.string().optional(),
});

const SearchResults = z.object({
  items: z
    .array(
      z.object({
        id: z.object({
          videoId: z.string().optional(),
          channelId: z.string().optional(),
          playlistId: z.string().optional(),
        }),
        snippet: z.object({
          title: z.string(),
          channelTitle: z.string().default(""),
          channelId: z.string().optional(),
          publishedAt: z.string().optional(),
        }),
      }),
    )
    .default([]),
  nextPageToken: z.string().optional(),
});

const Playlist = z.object({
  id: z.string(),
  snippet: z.object({ title: z.string(), description: z.string().default("") }),
  status: z.object({ privacyStatus: z.string().optional() }).default({}),
  contentDetails: z.object({ itemCount: z.number().optional() }).default({}),
});

const Playlists = z.object({
  items: z.array(Playlist).default([]),
  nextPageToken: z.string().optional(),
});

const PlaylistItems = z.object({
  items: z
    .array(
      z.object({
        id: z.string(),
        snippet: z.object({
          title: z.string(),
          position: z.number().optional(),
          videoOwnerChannelTitle: z.string().default(""),
          resourceId: z.object({ videoId: z.string().optional() }),
        }),
      }),
    )
    .default([]),
  nextPageToken: z.string().optional(),
});

const Subscriptions = z.object({
  items: z
    .array(
      z.object({
        snippet: z.object({
          title: z.string(),
          resourceId: z.object({ channelId: z.string() }),
        }),
      }),
    )
    .default([]),
  nextPageToken: z.string().optional(),
});

const paging = {
  maxResults: z.int().min(1).max(50).default(25),
  pageToken: z
    .string()
    .optional()
    .describe("nextPageToken from the previous call, to get the next page"),
};
const videoId = z.string().describe("A video id, such as one from youtube_search");

export const youtubeTools = {
  youtube_search: tool({
    description: `YouTube: search videos, channels, or playlists. Costs 100 times as much of the daily quota as any other YouTube tool, which all of goat's users share, so search only when the user asks to find something and no id is at hand. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      query: z.string(),
      type: z.enum(["video", "channel", "playlist"]).default("video"),
      ...paging,
      maxResults: z.int().min(1).max(10).default(5),
    }),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ query, type, maxResults, pageToken }, { context, abortSignal }) => {
      const found = SearchResults.parse(
        await get(
          context.fetch,
          "search",
          { part: "snippet", q: query, type, maxResults, pageToken },
          abortSignal,
        ),
      );
      return {
        results: found.items.flatMap(({ id, snippet }) => {
          const resultId = id.videoId ?? id.channelId ?? id.playlistId;
          if (resultId === undefined) return [];
          return {
            type,
            id: resultId,
            title: untrusted(snippet.title),
            channel: untrusted(snippet.channelTitle),
            channelId: snippet.channelId,
            publishedAt: snippet.publishedAt,
          };
        }),
        nextPageToken: found.nextPageToken,
      };
    },
  }),
  youtube_get_videos: tool({
    description: `YouTube: details of videos by id: title, channel, publish date, duration (ISO 8601), view and like counts, and the start of the description. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({ videoIds: z.array(videoId).min(1).max(10) }),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ videoIds }, { context, abortSignal }) => {
      const found = Videos.parse(
        await get(
          context.fetch,
          "videos",
          { part: videoParts, id: videoIds.join(",") },
          abortSignal,
        ),
      );
      return { videos: found.items.map((item) => describe(item, DESCRIPTION_CHARS)) };
    },
  }),
  youtube_list_liked_videos: tool({
    description: `YouTube: list the videos the user liked, newest first. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object(paging),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ maxResults, pageToken }, { context, abortSignal }) => {
      const found = Videos.parse(
        await get(
          context.fetch,
          "videos",
          { part: videoParts, myRating: "like", maxResults, pageToken },
          abortSignal,
        ),
      );
      return {
        videos: found.items.map((item) => describe(item, 0)),
        nextPageToken: found.nextPageToken,
      };
    },
  }),
  youtube_list_playlists: tool({
    description: "YouTube: list the user's own playlists with their ids and sizes.",
    inputSchema: z.object(paging),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ maxResults, pageToken }, { context, abortSignal }) => {
      const found = Playlists.parse(
        await get(
          context.fetch,
          "playlists",
          { part: "snippet,status,contentDetails", mine: true, maxResults, pageToken },
          abortSignal,
        ),
      );
      return {
        playlists: found.items.map(summarize),
        nextPageToken: found.nextPageToken,
      };
    },
  }),
  youtube_list_playlist_items: tool({
    description: `YouTube: list the videos in a playlist, in playlist order. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object({
      playlistId: z.string().describe("A playlist id from youtube_list_playlists"),
      ...paging,
    }),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ playlistId, maxResults, pageToken }, { context, abortSignal }) => {
      const found = PlaylistItems.parse(
        await get(
          context.fetch,
          "playlistItems",
          { part: "snippet", playlistId, maxResults, pageToken },
          abortSignal,
        ),
      );
      return {
        items: found.items.map(({ id, snippet }) => ({
          itemId: id,
          videoId: snippet.resourceId.videoId,
          title: untrusted(snippet.title),
          channel: untrusted(snippet.videoOwnerChannelTitle),
          position: snippet.position,
        })),
        nextPageToken: found.nextPageToken,
      };
    },
  }),
  youtube_list_subscriptions: tool({
    description: `YouTube: list the channels the user subscribes to. ${UNTRUSTED_NOTE}`,
    inputSchema: z.object(paging),
    contextSchema: GoogleConnection,
    metadata: readMetadata,
    execute: async ({ maxResults, pageToken }, { context, abortSignal }) => {
      const found = Subscriptions.parse(
        await get(
          context.fetch,
          "subscriptions",
          { part: "snippet", mine: true, maxResults, pageToken },
          abortSignal,
        ),
      );
      return {
        channels: found.items.map(({ snippet }) => ({
          channelId: snippet.resourceId.channelId,
          title: untrusted(snippet.title),
        })),
        nextPageToken: found.nextPageToken,
      };
    },
  }),
  youtube_create_playlist: tool({
    description: "YouTube: create a playlist on the user's channel. Private unless told otherwise.",
    inputSchema: z.object({
      title: z.string(),
      description: z.string().optional(),
      privacy: z.enum(["private", "unlisted", "public"]).default("private"),
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ title, description, privacy }, { context, abortSignal }) => {
      const created = Playlist.parse(
        await callGoogle(context.fetch, url("playlists", { part: "snippet,status" }), {
          method: "POST",
          body: { snippet: { title, description }, status: { privacyStatus: privacy } },
          signal: abortSignal,
        }),
      );
      return summarize(created);
    },
  }),
  youtube_add_to_playlist: tool({
    description: "YouTube: add a video to one of the user's playlists.",
    inputSchema: z.object({
      playlistId: z.string().describe("A playlist id from youtube_list_playlists"),
      videoId,
    }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ playlistId, videoId: video }, { context, abortSignal }) => {
      const added = z.object({ id: z.string() }).parse(
        await callGoogle(context.fetch, url("playlistItems", { part: "snippet" }), {
          method: "POST",
          body: {
            snippet: { playlistId, resourceId: { kind: "youtube#video", videoId: video } },
          },
          signal: abortSignal,
        }),
      );
      return { itemId: added.id, playlistId, videoId: video };
    },
  }),
  youtube_rate_video: tool({
    description: "YouTube: like or dislike a video as the user, or remove their rating.",
    inputSchema: z.object({ videoId, rating: z.enum(["like", "dislike", "none"]) }),
    contextSchema: GoogleConnection,
    metadata,
    execute: async ({ videoId: video, rating }, { context, abortSignal }) => {
      await callGoogle(context.fetch, url("videos/rate", { id: video, rating }), {
        method: "POST",
        signal: abortSignal,
      });
      return { videoId: video, rating };
    },
  }),
};

const videoParts = "snippet,contentDetails,statistics";

type Params = Record<string, string | number | boolean | undefined>;

function url(path: string, params: Params): URL {
  const result = new URL(`${API}/${path}`);
  for (const [name, value] of Object.entries(params)) {
    if (value !== undefined) result.searchParams.set(name, String(value));
  }
  return result;
}

function get(fetch: Fetch, path: string, params: Params, signal: AbortSignal | undefined) {
  return callGoogle(fetch, url(path, params), { signal });
}

function describe(video: z.infer<typeof Video>, descriptionChars: number) {
  const { snippet, statistics } = video;
  return {
    id: video.id,
    title: untrusted(snippet.title),
    channel: untrusted(snippet.channelTitle),
    channelId: snippet.channelId,
    publishedAt: snippet.publishedAt,
    duration: video.contentDetails.duration,
    views: statistics.viewCount,
    likes: statistics.likeCount,
    comments: statistics.commentCount,
    ...(descriptionChars === 0
      ? {}
      : {
          description: untrusted(snippet.description.slice(0, descriptionChars)),
          descriptionTruncated: snippet.description.length > descriptionChars,
        }),
  };
}

function summarize(playlist: z.infer<typeof Playlist>) {
  return {
    id: playlist.id,
    title: playlist.snippet.title,
    description: playlist.snippet.description,
    privacy: playlist.status.privacyStatus,
    itemCount: playlist.contentDetails.itemCount,
  };
}
