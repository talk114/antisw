import React, { useState } from 'react';
import { Lock, Key, Loader2, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

interface SudoPasswordDialogProps {
  isOpen: boolean;
  onConfirm: (password: string) => void;
  onCancel: () => void;
  title?: string;
  message?: string;
  isLoading?: boolean;
}

export const SudoPasswordDialog: React.FC<SudoPasswordDialogProps> = ({
  isOpen,
  onConfirm,
  onCancel,
  title,
  message,
  isLoading = false,
}) => {
  const { t } = useTranslation();
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (password.trim()) {
      onConfirm(password.trim());
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 backdrop-blur-sm">
      <div className="bg-white dark:bg-base-100 rounded-2xl shadow-2xl overflow-hidden w-full max-w-md mx-4 animate-in fade-in zoom-in duration-200">
        {/* Header */}
        <div className="relative px-6 pt-6 pb-4">
          <button
            onClick={onCancel}
            className="absolute top-4 right-4 p-1 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 rounded-lg transition-colors"
            disabled={isLoading}
          >
            <X className="w-5 h-5" />
          </button>
          <div className="w-12 h-12 bg-amber-50 dark:bg-amber-900/20 rounded-xl flex items-center justify-center mb-4">
            <Lock className="w-6 h-6 text-amber-500" />
          </div>
          <h2 className="text-xl font-bold text-gray-900 dark:text-white pr-8">
            {title || t('sudo_password.title', 'Yêu cầu mật khẩu Admin')}
          </h2>
        </div>

        {/* Body */}
        <form onSubmit={handleSubmit} className="px-6 pb-6">
          <p className="text-sm text-gray-500 dark:text-gray-400 mb-4">
            {message || t('sudo_password.message', 'Nhập mật khẩu sudo để cập nhật hệ thống.')}
          </p>

          <div className="relative mb-6">
            <Key className="absolute left-4 top-1/2 -translate-y-1/2 w-5 h-5 text-gray-400" />
            <input
              type={showPassword ? 'text' : 'password'}
              placeholder={t('sudo_password.placeholder', 'Mật khẩu máy tính')}
              className="w-full pl-12 pr-12 py-3 bg-gray-50 dark:bg-base-200 border-2 border-transparent rounded-xl focus:ring-2 focus:ring-amber-500 focus:border-amber-500 transition-all outline-none text-gray-900 dark:text-white"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              autoFocus
              disabled={isLoading}
            />
            <button
              type="button"
              onClick={() => setShowPassword(!showPassword)}
              className="absolute right-4 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
              tabIndex={-1}
            >
              {showPassword ? (
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
                </svg>
              ) : (
                <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                </svg>
              )}
            </button>
          </div>

          <div className="flex gap-3">
            <button
              type="button"
              onClick={onCancel}
              disabled={isLoading}
              className="flex-1 py-3 bg-gray-100 dark:bg-base-200 hover:bg-gray-200 dark:hover:bg-base-300 text-gray-700 dark:text-gray-300 font-medium rounded-xl transition-colors disabled:opacity-50"
            >
              {t('common.cancel', 'Hủy')}
            </button>
            <button
              type="submit"
              disabled={!password.trim() || isLoading}
              className="flex-1 py-3 bg-amber-500 hover:bg-amber-600 disabled:bg-amber-300 text-white font-bold rounded-xl shadow-lg shadow-amber-500/30 transition-all active:scale-[0.98] disabled:scale-100 flex items-center justify-center gap-2"
            >
              {isLoading ? (
                <>
                  <Loader2 className="w-5 h-5 animate-spin" />
                  {t('sudo_password.btn_confirming', 'Đang xác nhận...')}
                </>
              ) : (
                t('sudo_password.btn_confirm', 'Xác nhận')
              )}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};