import { tv, type VariantProps } from "tailwind-variants";

/**
 * shadcn's button recipe, kept in a plain module because Astro components
 * cannot export values.
 */
export const buttonVariants = tv({
  base:
    "inline-flex shrink-0 items-center justify-center gap-2 rounded-md text-sm font-medium whitespace-nowrap " +
    "outline-none transition-[color,box-shadow,background-color,border-color] " +
    "focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:border-ring " +
    "disabled:pointer-events-none disabled:opacity-50 " +
    "[&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  variants: {
    variant: {
      default:
        "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90",
      accent: "bg-accent text-accent-foreground shadow-xs hover:bg-accent/90",
      outline:
        "border bg-card shadow-xs hover:bg-secondary hover:text-secondary-foreground",
      ghost: "hover:bg-secondary hover:text-secondary-foreground",
      link: "text-primary underline-offset-4 hover:underline",
    },
    size: {
      default: "h-9 px-4 py-2 has-[>svg]:px-3",
      sm: "h-8 gap-1.5 rounded-md px-3 has-[>svg]:px-2.5",
      lg: "h-11 rounded-md px-6 text-[0.9375rem] has-[>svg]:px-5",
    },
  },
  defaultVariants: { variant: "default", size: "default" },
});

export type ButtonVariant = VariantProps<typeof buttonVariants>["variant"];
export type ButtonSize = VariantProps<typeof buttonVariants>["size"];
