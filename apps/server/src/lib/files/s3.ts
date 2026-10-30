import { S3Client } from "bun";
import { env } from "../../env";

const client = new S3Client({
  endpoint: env.S3_ENDPOINT,
  bucket: env.S3_BUCKET,
  region: env.S3_REGION,
  accessKeyId: env.S3_ACCESS_KEY_ID,
  secretAccessKey: env.S3_SECRET_ACCESS_KEY,
});

export async function put(key: string, data: Uint8Array, mediaType: string): Promise<void> {
  await client.write(key, data, { type: mediaType });
}

export function get(key: string): Promise<Uint8Array> {
  return client.file(key).bytes();
}

export async function remove(key: string): Promise<void> {
  await client.delete(key);
}

export function open(key: string): Blob {
  return client.file(key);
}
