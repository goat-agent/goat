import { AutoModel, AutoTokenizer, env as transformers, Tensor } from "@huggingface/transformers";
import { env } from "../../env";

/** Stored beside each vector; a round embedded by another model counts as not embedded. */
export const embeddingModel = "embeddinggemma-300m";

const REPOSITORY = "onnx-community/embeddinggemma-300m-ONNX";
const MAX_TOKENS = 1024;
const BATCH = 8;
// The prompts the model was trained with; without them quality drops.
const QUERY_PREFIX = "task: search result | query: ";
const DOCUMENT_PREFIX = "title: none | text: ";

transformers.cacheDir = env.MODEL_CACHE_DIR;

type Loaded = {
  tokenizer: Awaited<ReturnType<typeof AutoTokenizer.from_pretrained>>;
  model: Awaited<ReturnType<typeof AutoModel.from_pretrained>>;
};

// Files download on first use; a failed load is not remembered, so the next call tries again.
let loading: Promise<Loaded> | undefined;

function load(): Promise<Loaded> {
  loading ??= Promise.all([
    AutoTokenizer.from_pretrained(REPOSITORY),
    AutoModel.from_pretrained(REPOSITORY, { dtype: "q8" }),
  ]).then(
    ([tokenizer, model]) => ({ tokenizer, model }),
    (error: unknown) => {
      loading = undefined;
      throw error;
    },
  );
  return loading;
}

export async function embedDocuments(texts: readonly string[]): Promise<number[][]> {
  return embed(texts.map((text) => DOCUMENT_PREFIX + text));
}

export async function embedQuery(text: string): Promise<number[]> {
  const [vector] = await embed([QUERY_PREFIX + text]);
  if (!vector) throw new Error("the embedder returned no vector");
  return vector;
}

async function embed(texts: readonly string[]): Promise<number[][]> {
  const { tokenizer, model } = await load();
  const vectors: number[][] = [];
  for (let start = 0; start < texts.length; start += BATCH) {
    const inputs = tokenizer(texts.slice(start, start + BATCH), {
      padding: true,
      truncation: true,
      max_length: MAX_TOKENS,
    });
    const output: unknown = await model(inputs);
    const embedding =
      typeof output === "object" && output !== null && "sentence_embedding" in output
        ? output.sentence_embedding
        : undefined;
    if (!(embedding instanceof Tensor) || !(embedding.data instanceof Float32Array)) {
      throw new Error("the embedder returned no sentence embedding");
    }
    const [rows = 0, width = 0] = embedding.dims;
    const { data } = embedding;
    for (let row = 0; row < rows; row++) {
      vectors.push(normalized(data.subarray(row * width, (row + 1) * width)));
    }
  }
  return vectors;
}

function normalized(vector: Float32Array): number[] {
  const length = Math.sqrt(vector.reduce((sum, x) => sum + x * x, 0));
  return Array.from(vector, (x) => x / length);
}
