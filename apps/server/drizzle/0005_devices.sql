CREATE TABLE "device_calls" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"device_id" uuid NOT NULL,
	"method" text NOT NULL,
	"params" jsonb NOT NULL,
	"picked_at" timestamp with time zone,
	"response" jsonb,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL
);
--> statement-breakpoint
CREATE TABLE "devices" (
	"id" uuid PRIMARY KEY DEFAULT uuidv7() NOT NULL,
	"user_id" text NOT NULL,
	"installation_id" uuid NOT NULL,
	"name" text NOT NULL,
	"platform" text NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	CONSTRAINT "devices_userId_installationId_unique" UNIQUE("user_id","installation_id")
);
--> statement-breakpoint
ALTER TABLE "sessions" ADD COLUMN "device_id" text;--> statement-breakpoint
ALTER TABLE "device_calls" ADD CONSTRAINT "device_calls_device_id_devices_id_fk" FOREIGN KEY ("device_id") REFERENCES "public"."devices"("id") ON DELETE cascade ON UPDATE no action;--> statement-breakpoint
ALTER TABLE "devices" ADD CONSTRAINT "devices_user_id_users_id_fk" FOREIGN KEY ("user_id") REFERENCES "public"."users"("id") ON DELETE cascade ON UPDATE no action;