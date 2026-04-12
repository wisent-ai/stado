import { AuthProvider } from '@/contexts/AuthContext';
import { Header } from '@/components/layout/Header';

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return (
    <AuthProvider>
      <div className="min-h-screen bg-background">
        <Header />
        <main className="container mx-auto px-4 py-6">{children}</main>
      </div>
    </AuthProvider>
  );
}
