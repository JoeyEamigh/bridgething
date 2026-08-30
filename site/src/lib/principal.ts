export type JudgeRecord = { handle: string; created_at: string };

export type Principal = { role: 'admin' } | { role: 'judge'; handle: string };
