execute if score #wsc arnis.ds matches 1 run function arnis:disaster/water/rule {rule:"waterSourceConversion",value:"true"}
execute if score #wsc arnis.ds matches 1 run function arnis:disaster/water/rule {rule:"water_source_conversion",value:"true"}
scoreboard players reset #wsc arnis.ds
scoreboard players set #wsc_idle arnis.ds 0
