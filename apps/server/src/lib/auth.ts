import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { bearer, emailOTP } from "better-auth/plugins";
import { Resend } from "resend";
import { db } from "../db";
import { env } from "../env";
import { localeOf, type Locale } from "./i18n";

const DAY = 60 * 60 * 24;
const resend = new Resend(env.RESEND_API_KEY);

const codeEmail: Record<Locale, (otp: string) => { subject: string; text: string }> = {
  en: (otp) => ({ subject: "Your Goat sign-in code", text: `Your sign-in code is ${otp}.` }),
  ko: (otp) => ({ subject: "Goat 로그인 코드", text: `로그인 코드는 ${otp}입니다.` }),
};

export const auth = betterAuth({
  baseURL: env.BETTER_AUTH_URL,
  secret: env.BETTER_AUTH_SECRET,
  database: drizzleAdapter(db, { provider: "pg", usePlural: true }),
  session: { expiresIn: 90 * DAY, updateAge: DAY },
  user: { additionalFields: { timeZone: { type: "string", required: false } } },
  plugins: [
    bearer(),
    emailOTP({
      async sendVerificationOTP({ email, otp }, ctx) {
        const { error } = await resend.emails.send({
          from: env.EMAIL_FROM,
          to: email,
          ...codeEmail[localeOf(ctx?.headers)](otp),
        });
        if (error) throw new Error(error.message);
      },
    }),
  ],
});
