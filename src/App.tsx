import { useEffect, useState } from "react";
import { getAppInfo } from "./lib/version";

function App() {
  const [version, setVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getAppInfo()
      .then((info) => setVersion(info.version))
      .catch((err: unknown) =>
        setError(err instanceof Error ? err.message : String(err)),
      );
  }, []);

  return (
    <main className="flex min-h-screen flex-col items-center justify-center bg-neutral-900 text-neutral-100">
      <h1 className="text-2xl font-semibold">Archimedes Desktop</h1>
      {error ? (
        <p className="mt-4 text-sm text-red-400">{error}</p>
      ) : version ? (
        <p className="mt-4 text-lg">Version {version}</p>
      ) : (
        <p className="mt-4 text-sm text-neutral-400">Loading…</p>
      )}
    </main>
  );
}

export default App;
