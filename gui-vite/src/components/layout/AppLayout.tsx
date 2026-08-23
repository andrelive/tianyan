import { Outlet } from 'react-router-dom';
import Sidebar from '@/components/sidebar/Sidebar';
import ClipboardConfirmBar from '@/components/clipboard/ClipboardConfirmBar';
export default function AppLayout() {
  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar />
      <main className="flex-1 flex flex-col overflow-hidden bg-[var(--color-bg-primary)]">
        <Outlet />
      </main>
      <ClipboardConfirmBar />
    </div>
  );
}
