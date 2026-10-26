execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/quake/hit
# New cracks open every few seconds, up to the aftershock.
scoreboard players operation #q arnis.ds = #len arnis.ds
scoreboard players operation #q arnis.ds -= #t arnis.ds
execute if score #q arnis.ds matches 6.. if score #t arnis.ds >= #qk_next arnis.ds run function arnis:disaster/quake/fissures
scoreboard players operation #q arnis.ds = #t arnis.ds
scoreboard players operation #q arnis.ds %= #3 arnis
execute if score #q arnis.ds matches 0 in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.warden.dig ambient @s ~ ~-3 ~ 1.5 0.5
