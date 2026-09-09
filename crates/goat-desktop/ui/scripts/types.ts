import { compile, type JSONSchema } from 'json-schema-to-typescript';
import { mkdir, readFile, writeFile } from 'node:fs/promises';

const source = new URL('../../../goat-api/src/methods_schema.json', import.meta.url);
const target = new URL('../src/lib/api.d.ts', import.meta.url);
type Schema = boolean | { [key: string]: unknown };
type Registry = { methods: { name: string; params: Schema; output: Schema; item: Schema }[] };
const registry: Registry = JSON.parse(await readFile(source, 'utf8'));
const definitions: Record<string, Schema> = {};

function clean(value: unknown, propertyMap = false): unknown {
  if (Array.isArray(value)) return value.map(child => clean(child));
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value)
      .filter(([key]) => propertyMap || !['description', '$comment', '$schema', 'title', '$defs'].includes(key))
      .map(([key, child]) => [key, !propertyMap && key === '$ref' && typeof child === 'string'
        ? child.replace('#/$defs/', '#/definitions/')
        : clean(child, !propertyMap && ['properties', 'patternProperties', 'dependentSchemas', 'definitions'].includes(key))]));
  }
  return value;
}

function add(name: string, value: Schema) {
  const normalized = clean(value) as Schema;
  if (definitions[name] && JSON.stringify(definitions[name]) !== JSON.stringify(normalized)) {
    throw new Error(`Conflicting API schema definition: ${name}`);
  }
  definitions[name] = normalized;
}

for (const method of registry.methods) {
  for (const part of [method.params, method.output, method.item]) {
    if (typeof part === 'boolean') continue;
    for (const [name, schema] of Object.entries((part.$defs ?? {}) as Record<string, Schema>)) {
      add(name, schema);
    }
    if (typeof part.title === 'string' && part.title !== 'null') add(part.title, part);
  }
}

const names = Object.keys(definitions).sort();
const root = {
  title: 'ApiSchema',
  type: 'object',
  additionalProperties: false,
  required: names,
  properties: Object.fromEntries(names.map(name => [name, { $ref: `#/definitions/${name}` }])),
  definitions,
};
const declarations = await compile(root as JSONSchema, 'ApiSchema', {
  bannerComment: '',
  additionalProperties: false,
  unknownAny: true,
  ignoreMinAndMaxItems: true,
  format: false,
  style: { singleQuote: true },
});
await mkdir(new URL('../src/lib/', import.meta.url), { recursive: true });
await writeFile(target, declarations);
