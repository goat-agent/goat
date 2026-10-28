import { betterAuth } from "better-auth";
import { drizzleAdapter } from "better-auth/adapters/drizzle";
import { bearer, emailOTP } from "better-auth/plugins";
import { Resend } from "resend";
import { db } from "../db";
import { env } from "../env";

const DAY = 60 * 60 * 24;
const resend = new Resend(env.RESEND_API_KEY);

export const auth = betterAuth({
  baseURL: env.BETTER_AUTH_URL,
  secret: env.BETTER_AUTH_SECRET,
  database: drizzleAdapter(db, { provider: "pg", usePlural: true }),
  session: { expiresIn: 90 * DAY, updateAge: DAY },
  plugins: [
    bearer(),
    emailOTP({
      async sendVerificationOTP({ email, otp }) {
        const { error } = await resend.emails.send({
          from: env.EMAIL_FROM,
          to: email,
          subject: "Goat 로그인 코드",
          text: `로그인 코드는 ${otp}입니다.`,
        });
        if (error) throw new Error(error.message);
      },
    }),
  ],
});
