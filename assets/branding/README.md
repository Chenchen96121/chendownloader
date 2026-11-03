# chen icon

The application uses a white lowercase `chen` wordmark on a blue background.

- `chen-source.png`: editable export source.
- `public/chen-icon.png`: application/sidebar image.
- `src-tauri/icons/chen.ico`: multi-resolution Windows icon.

Generate icon exports with the project's Tauri CLI:

```powershell
npm run tauri -- icon assets/branding/chen-source.png --output .release/chen-icons
```
