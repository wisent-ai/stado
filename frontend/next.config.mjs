/** @type {import('next').NextConfig} */
const nextConfig = {
  output: 'standalone',
  transpilePackages: ['@wisent-ai/onboarding-web'],
  experimental: {
    serverActions: true,
  },
};

export default nextConfig;
