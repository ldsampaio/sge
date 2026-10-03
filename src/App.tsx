import LoginForm from "./components/LoginForm";
import "./App.css";

function isTauriRuntime(): boolean {
  const w = window as unknown as Record<string, unknown>;
  return w["__TAURI_INTERNALS__"] !== undefined || w["__TAURI__"] !== undefined;
}

function App() {
  const outsideDesktop = !isTauriRuntime();
  return (
    <main className="container">
      <h1>SGE</h1>
      {outsideDesktop && (
        <p role="alert">
          SGE is running outside its desktop window — launch with `npm run
          tauri dev` and use the app window, not the browser URL.
        </p>
      )}
      <LoginForm />
    </main>
  );
}

export default App;
