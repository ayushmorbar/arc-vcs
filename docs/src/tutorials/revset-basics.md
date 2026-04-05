# Tutorial: First Useful Revset

## Goal

Build one revset that finds likely causative changes for a file-level regression.

---

## Step 1: Inspect current ancestry

```bash
arc log -r 'ancestors(@)'
```

## Step 2: Filter by path impact

```bash
arc log -r 'ancestors(@) & touched("src/main.rs")'
```

You now have a focused candidate set.

## Step 3: Add release anchors

```bash
arc log -r 'ancestors(@) & touched("src/main.rs") & (bookmarks() | tags())'
```

This highlights changes tied to branch/tag anchors.

## Step 4: Inspect one candidate deeply

```bash
arc show <change-id>
```

---

## Result

You can now compose revsets for targeted investigation instead of scanning long logs manually.
