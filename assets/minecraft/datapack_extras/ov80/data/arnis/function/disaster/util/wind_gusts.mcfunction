execute store result score #gust arnis.ds run random value 50..90
execute if score #pass arnis.ds matches 2.. store result score #gust arnis.ds run random value 35..70
execute in minecraft:overworld as @a[gamemode=!spectator,distance=0..] at @s run function arnis:disaster/util/wind_gust
