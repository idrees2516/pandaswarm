import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";
import { Toaster } from "@/components/ui/toaster";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "PandaSwarm — Agentic Coding-Agent Orchestration",
  description: "Spawn agent swarms for coding tasks: three-tier cowork orchestration, self-hosted Rust Durable Objects, layered sandboxes (local + instabox.sh), hybrid RAG and semantic memory.",
  keywords: ["PandaSwarm", "AI agents", "orchestration", "Rust", "Durable Objects", "sandbox", "RAG"],
  authors: [{ name: "PandaSwarm" }],
  icons: {
    icon: "https://z-cdn.chatglm.cn/z-ai/static/logo.svg",
  },
  openGraph: {
    title: "PandaSwarm",
    description: "Agentic coding-agent orchestration",
    siteName: "PandaSwarm",
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "PandaSwarm",
    description: "Agentic coding-agent orchestration",
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased bg-background text-foreground`}
      >
        {children}
        <Toaster />
      </body>
    </html>
  );
}
