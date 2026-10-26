bossbar set arnis:disaster players @a
execute if score #phase arnis.ds matches 1 run function arnis:disaster/hud/calm
execute if score #phase arnis.ds matches 2 run function arnis:disaster/hud/warn
execute if score #phase arnis.ds matches 3 run function arnis:disaster/hud/active
