CREATE TABLE "files" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"agent_id" uuid,
	"name" text NOT NULL,
	"media_type" text NOT NULL,
	"sent_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "files" ADD CONSTRAINT "files_agent_id_agents_id_fk" FOREIGN KEY ("agent_id") REFERENCES "public"."agents"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "files_agent_id_index" ON "files" USING btree ("agent_id");--> statement-breakpoint
CREATE INDEX "files_created_at_index" ON "files" USING btree ("created_at") WHERE "files"."sent_at" is null or "files"."agent_id" is null;