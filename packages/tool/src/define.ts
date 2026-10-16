import type { JsonSchema } from "@goat/provider";
import { z } from "zod";
import type { Tool, ToolContext, ToolOutput } from "./tool.ts";

export interface ToolSpec<Shape extends z.core.$ZodShape> {
  readonly name: string;
  readonly description: string;
  readonly input: z.ZodObject<Shape>;
  run(input: z.output<z.ZodObject<Shape>>, context: ToolContext): Promise<ToolOutput>;
}

export function defineTool<Shape extends z.core.$ZodShape>(spec: ToolSpec<Shape>): Tool {
  return {
    definition: {
      type: "function",
      name: spec.name,
      description: spec.description,
      inputSchema: inputSchemaOf(spec.input),
    },
    call: async (input, context) => {
      const parsed = spec.input.safeParse(input);
      if (!parsed.success) {
        return {
          parts: [
            {
              type: "text",
              text: `The input does not match the schema.\n${z.prettifyError(parsed.error)}`,
            },
          ],
          isError: true,
        };
      }
      return spec.run(parsed.data, context);
    },
  };
}

const jsonSchema = z.record(z.string(), z.json());

function inputSchemaOf(input: z.ZodType): JsonSchema {
  const generated = z.toJSONSchema(input, { io: "input" });
  return jsonSchema.parse(
    Object.fromEntries(
      Object.entries(generated).filter(
        (entry: readonly [string, unknown]) => entry[0] !== "$schema",
      ),
    ),
  );
}
