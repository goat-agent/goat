CREATE TABLE "job_runs" (
	"id" bigint PRIMARY KEY GENERATED ALWAYS AS IDENTITY (sequence name "job_runs_id_seq" INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 1 CACHE 1),
	"job_id" uuid NOT NULL,
	"scheduled_for" timestamp with time zone NOT NULL,
	"owner" uuid,
	"lease_until" timestamp with time zone,
	"attempts" integer DEFAULT 0 NOT NULL,
	"finished_at" timestamp with time zone,
	"error" text,
	"outcome" text,
	"model" text,
	"usage" jsonb,
	"message_id" bigint,
	CONSTRAINT "job_runs_jobId_scheduledFor_unique" UNIQUE("job_id","scheduled_for")
);
--> statement-breakpoint
CREATE TABLE "jobs" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"agent_id" uuid NOT NULL,
	"title" text NOT NULL,
	"instruction" text NOT NULL,
	"start" timestamp NOT NULL,
	"rrule" text,
	"enabled" boolean DEFAULT true NOT NULL,
	"next_run_at" timestamp with time zone,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "memory_entries" (
	"id" bigint PRIMARY KEY GENERATED ALWAYS AS IDENTITY (sequence name "memory_entries_id_seq" INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 1 CACHE 1),
	"page_id" uuid NOT NULL,
	"text" text NOT NULL,
	"until" date,
	"past_at" timestamp with time zone,
	"replaced_by" bigint,
	"source_agent_id" uuid,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "memory_pages" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"user_id" text NOT NULL,
	"kind" text NOT NULL,
	"title" text NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "rounds" (
	"id" bigint PRIMARY KEY GENERATED ALWAYS AS IDENTITY (sequence name "rounds_id_seq" INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 1 CACHE 1),
	"agent_id" uuid NOT NULL,
	"first_message_id" bigint NOT NULL,
	"last_message_id" bigint NOT NULL,
	"started_at" timestamp with time zone NOT NULL,
	"ended_at" timestamp with time zone NOT NULL,
	"user_text" text NOT NULL,
	"reply_text" text NOT NULL,
	"search_text" text NOT NULL,
	"embedding" vector(768),
	"embedded_model" text,
	"embed_claimed_until" timestamp with time zone,
	"embed_attempts" integer DEFAULT 0 NOT NULL,
	"updated_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "summaries" (
	"id" bigint PRIMARY KEY GENERATED ALWAYS AS IDENTITY (sequence name "summaries_id_seq" INCREMENT BY 1 MINVALUE 1 MAXVALUE 9223372036854775807 START WITH 1 CACHE 1),
	"agent_id" uuid NOT NULL,
	"upto_message_id" bigint NOT NULL,
	"text" text NOT NULL,
	"notes" jsonb NOT NULL,
	"computer_id_high" integer NOT NULL,
	"complete" boolean NOT NULL,
	"model" text NOT NULL,
	"usage" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
ALTER TABLE "agents" ADD COLUMN "schedules_note" text;--> statement-breakpoint
ALTER TABLE "agents" ADD COLUMN "memory_note" text;--> statement-breakpoint
ALTER TABLE "agents" ADD COLUMN "summary_redactions" text[] DEFAULT '{}'::text[] NOT NULL;--> statement-breakpoint
ALTER TABLE "devices" ADD COLUMN "push_token" text;--> statement-breakpoint
ALTER TABLE "job_runs" ADD CONSTRAINT "job_runs_job_id_jobs_id_fk" FOREIGN KEY ("job_id") REFERENCES "public"."jobs"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "job_runs" ADD CONSTRAINT "job_runs_message_id_messages_id_fk" FOREIGN KEY ("message_id") REFERENCES "public"."messages"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "jobs" ADD CONSTRAINT "jobs_agent_id_agents_id_fk" FOREIGN KEY ("agent_id") REFERENCES "public"."agents"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "memory_entries" ADD CONSTRAINT "memory_entries_page_id_memory_pages_id_fk" FOREIGN KEY ("page_id") REFERENCES "public"."memory_pages"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "memory_entries" ADD CONSTRAINT "memory_entries_source_agent_id_agents_id_fk" FOREIGN KEY ("source_agent_id") REFERENCES "public"."agents"("id") ON DELETE set null ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "memory_pages" ADD CONSTRAINT "memory_pages_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "rounds" ADD CONSTRAINT "rounds_agent_id_agents_id_fk" FOREIGN KEY ("agent_id") REFERENCES "public"."agents"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "summaries" ADD CONSTRAINT "summaries_agent_id_agents_id_fk" FOREIGN KEY ("agent_id") REFERENCES "public"."agents"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
CREATE INDEX "job_runs_job_id_index" ON "job_runs" USING btree ("job_id") WHERE "job_runs"."finished_at" is null;--> statement-breakpoint
CREATE INDEX "jobs_agent_id_index" ON "jobs" USING btree ("agent_id");--> statement-breakpoint
CREATE INDEX "jobs_next_run_at_index" ON "jobs" USING btree ("next_run_at") WHERE "jobs"."enabled" and "jobs"."next_run_at" is not null;--> statement-breakpoint
CREATE INDEX "memory_entries_page_id_index" ON "memory_entries" USING btree ("page_id");--> statement-breakpoint
CREATE UNIQUE INDEX "memory_pages_user_id_kind_title_unique" ON "memory_pages" USING btree ("user_id","kind",lower("title"));--> statement-breakpoint
CREATE UNIQUE INDEX "rounds_agent_id_first_message_id_index" ON "rounds" USING btree ("agent_id","first_message_id");--> statement-breakpoint
CREATE INDEX "rounds_agent_id_started_at_index" ON "rounds" USING btree ("agent_id","started_at");--> statement-breakpoint
CREATE INDEX "rounds_search_text_index" ON "rounds" USING gin ("search_text" gin_trgm_ops);--> statement-breakpoint
CREATE INDEX "rounds_embedding_index" ON "rounds" USING hnsw ("embedding" vector_cosine_ops) WHERE "rounds"."embedding" is not null;--> statement-breakpoint
CREATE INDEX "summaries_agent_id_id_index" ON "summaries" USING btree ("agent_id","id");