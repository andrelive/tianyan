import { Outlet } from 'react-router-dom';
import Sidebar from '@/components/sidebar/Sidebar';
import { useAppStore } from '@/lib/store';

export default function AppLayout() {
  const isSidebarOpen = useAppStore((s) => s.isSidebarOpen);

  return (
    <div className="flex h-screen overflow-hidden">
      <Sidebar />
      <main className="flex-1 flex flex-col overflow-hidden bg-[var(--color-bg-primary)]">
        <Outlet />
      </main>
    </div>
  );
}
