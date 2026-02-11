import { Link } from 'react-router-dom';
import LogoIcon from '../../../src-tauri/icons/icon.png';

/**
 * Logo 组件 - 独立处理响应式
 * 
 * 响应式策略:
 * 父容器宽度 >= 200px: Logo + 文字
 * 父容器宽度 <  200px: 只有 Logo
 */
export function NavLogo() {
    return (
        <Link to="/" draggable="false" className="flex w-full min-w-0 items-center gap-2 text-xl font-semibold text-gray-900 dark:text-base-content">
            <img src={LogoIcon} alt="Logo" className="w-8 h-8" draggable="false" />
            {/* 820px 以下隐藏文字 */}
            <span className="hidden min-[820px]:inline">AntiSwitcher</span>
        </Link>
    );
}
