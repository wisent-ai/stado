'use client';

import Link from 'next/link';
import { useAuth } from '@/contexts/AuthContext';
import { Button } from '@/components/ui/button';

export function Header() {
  const { user, signOut } = useAuth();

  return (
    <header className="border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
      <div className="container flex h-14 items-center justify-between px-4">
        <div className="flex items-center gap-6">
          <Link href="/" className="font-bold">Wisent Compute</Link>
          <nav className="hidden items-center gap-4 text-sm md:flex">
            <Link href="/marketplace" className="text-muted-foreground hover:text-foreground">Marketplace</Link>
            {user && (
              <>
                <Link href="/dashboard" className="text-muted-foreground hover:text-foreground">Dashboard</Link>
                <Link href="/instances" className="text-muted-foreground hover:text-foreground">Instances</Link>
                <Link href="/billing" className="text-muted-foreground hover:text-foreground">Billing</Link>
                <Link href="/host" className="text-muted-foreground hover:text-foreground">Host</Link>
              </>
            )}
          </nav>
        </div>
        <div className="flex items-center gap-2">
          {user ? (
            <>
              <span className="text-sm text-muted-foreground">{user.email}</span>
              <Button variant="ghost" size="sm" onClick={signOut}>Sign out</Button>
            </>
          ) : (
            <Link href="/login"><Button size="sm">Sign in</Button></Link>
          )}
        </div>
      </div>
    </header>
  );
}
