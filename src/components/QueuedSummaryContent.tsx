/**
 * QueuedSummaryContent.tsx
 *
 * A framer-motion based component for displaying a live-updating summary
 * of ongoing agent work.
 */

import React from "react";
import { motion } from "motion/react";

interface QueuedSummaryContentProps {
  children: React.ReactNode;
}

export function QueuedSummaryContent({ children }: QueuedSummaryContentProps) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -10 }}
      transition={{ duration: 0.2 }}
    >
      {children}
    </motion.div>
  );
}
