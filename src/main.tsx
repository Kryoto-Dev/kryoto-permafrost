import { createRoot } from 'react-dom/client'
import { App } from './App'
import './styles.css'
import { PixelIconMotion } from '@/ui/pixel-icons-motion'

createRoot(document.getElementById('root')!).render(<><PixelIconMotion /><App /></>)
