export function youtubePreviewUrls(videoId: string): { thumbnail: string; embed: string } | undefined {
  if (!/^[A-Za-z0-9_-]{11}$/.test(videoId)) return undefined;
  return {
    thumbnail: `https://i.ytimg.com/vi/${videoId}/hqdefault.jpg`,
    embed: `https://www.youtube-nocookie.com/embed/${videoId}?autoplay=1`,
  };
}
