import Link from 'next/link';
import { Button } from '@/components/ui/button';

export default function LandingPage() {
  return (
    <div className="min-h-screen bg-background">
      <header className="border-b">
        <div className="container mx-auto flex h-16 items-center justify-between px-4">
          <div className="flex items-center gap-2">
            <span className="text-xl font-bold">Wisent Compute</span>
          </div>
          <nav className="flex items-center gap-4">
            <Link href="/marketplace" className="text-sm text-muted-foreground hover:text-foreground">
              Marketplace
            </Link>
            <Link href="/login">
              <Button variant="ghost" size="sm">Log in</Button>
            </Link>
            <Link href="/signup">
              <Button size="sm">Get Started</Button>
            </Link>
          </nav>
        </div>
      </header>

      <main>
        <section className="container mx-auto px-4 py-24 text-center">
          <h1 className="text-5xl font-bold tracking-tight sm:text-7xl">
            GPU Compute
            <br />
            <span className="text-primary">Marketplace</span>
          </h1>
          <p className="mx-auto mt-6 max-w-2xl text-lg text-muted-foreground">
            Rent GPU machines from hosts worldwide. Run ML training, inference, and research
            at a fraction of cloud prices. List your own GPUs and earn.
          </p>
          <div className="mt-10 flex items-center justify-center gap-4">
            <Link href="/marketplace">
              <Button size="lg">Browse GPUs</Button>
            </Link>
            <Link href="/signup">
              <Button variant="outline" size="lg">Host Your GPUs</Button>
            </Link>
          </div>
        </section>

        <section className="border-t bg-muted/30 py-16">
          <div className="container mx-auto grid gap-8 px-4 md:grid-cols-3">
            <div className="text-center">
              <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-lg bg-primary/10">
                <span className="text-2xl">$</span>
              </div>
              <h3 className="font-semibold">Lowest Prices</h3>
              <p className="mt-2 text-sm text-muted-foreground">
                GPUs from $0.10/hr. Save up to 80% compared to major cloud providers.
              </p>
            </div>
            <div className="text-center">
              <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-lg bg-primary/10">
                <span className="text-2xl">~</span>
              </div>
              <h3 className="font-semibold">Instant Access</h3>
              <p className="mt-2 text-sm text-muted-foreground">
                SSH into your GPU machine in seconds. Pre-configured Docker images available.
              </p>
            </div>
            <div className="text-center">
              <div className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-lg bg-primary/10">
                <span className="text-2xl">+</span>
              </div>
              <h3 className="font-semibold">Earn as a Host</h3>
              <p className="mt-2 text-sm text-muted-foreground">
                List your idle GPUs and earn. Install the agent, set your price, start earning.
              </p>
            </div>
          </div>
        </section>
      </main>

      <footer className="border-t py-8">
        <div className="container mx-auto px-4 text-center text-sm text-muted-foreground">
          Wisent AI - compute.wisent.com
        </div>
      </footer>
    </div>
  );
}
