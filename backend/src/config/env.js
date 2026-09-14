import 'dotenv/config';
import { cleanEnv, str } from 'envalid';

const isProduction = process.env.NODE_ENV === 'production';

export function validateEnv() {
  return cleanEnv(
    process.env,
    {
      DATABASE_URL: str({ default: 'sqlite://data/soroban.db' }),
      REDIS_URL: str({ default: 'redis://localhost:6379' }),
      JWT_SECRET: str({ default: 'soroban-playground-secret-key-2026' }),
      SOROBAN_RPC_URL: str({ default: 'https://soroban-testnet.stellar.org' }),
      CORS_ALLOWED_ORIGINS: str({ default: '*' }),
    },
    {
      reporter: ({ errors }) => {
        const lines = Object.keys(errors).map(
          (name) => `  ${name}: ${errors[name].message}`
        );
        return `Invalid environment variables:\n${lines.join('\n')}`;
      },
    }
  );
}

let env;
try {
  env = validateEnv();
} catch (err) {
  if (process.env.NODE_ENV !== 'test') {
    console.warn('[env] Environment validation warning:', err.message);
  }
}

export default env;
