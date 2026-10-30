CREATE TABLE "connection_attempts" (
	"state" text PRIMARY KEY NOT NULL,
	"user_id" text NOT NULL,
	"integration_id" text NOT NULL,
	"agent_id" uuid,
	"code_verifier" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "connections" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"user_id" text NOT NULL,
	"integration_id" text NOT NULL,
	"account_id" text NOT NULL,
	"account_name" text NOT NULL,
	"credentials" text NOT NULL,
	"needs_reconnect" boolean DEFAULT false NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "connections_userId_integrationId_accountId_unique" UNIQUE("user_id","integration_id","account_id")
);
--> statement-breakpoint
ALTER TABLE "agents" ADD COLUMN "connections_note" text;--> statement-breakpoint
ALTER TABLE "messages" ADD COLUMN "hidden" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "queued_messages" ADD COLUMN "hidden" boolean DEFAULT false NOT NULL;--> statement-breakpoint
ALTER TABLE "connection_attempts" ADD CONSTRAINT "connection_attempts_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "connection_attempts" ADD CONSTRAINT "connection_attempts_agent_id_agents_id_fk" FOREIGN KEY ("agent_id") REFERENCES "public"."agents"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "connections" ADD CONSTRAINT "connections_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;